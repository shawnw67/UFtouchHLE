/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Audio Unit graph topology. Audio processing is implemented separately.

use std::collections::{BTreeMap, HashMap};

use super::{audio_components, audio_unit};
use crate::abi::GuestFunction;
use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::frameworks::carbon_core::{paramErr, OSStatus};
use crate::mem::{guest_size_of, ConstPtr, ConstVoidPtr, MutPtr, SafeRead};
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
    mixer: Option<(AUNode, AudioUnit)>,
    converters: BTreeMap<AUNode, AudioUnit>,
    initialized: bool,
    render_notifies: Vec<(GuestFunction, ConstVoidPtr)>,
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
    let connection = Connection {
        source,
        _source_bus: source_bus,
        destination,
        destination_bus,
    };
    let (opened, mixer, converter, output) = {
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
            || state.initialized
            || state
                .connections
                .iter()
                .any(|c| c.destination == destination && c.destination_bus == destination_bus)
        {
            return paramErr;
        }
        let opened = state.unit.is_some();
        if opened {
            state.connections.push(connection);
            let valid = analyze_graph(state, false).is_some();
            state.connections.pop();
            if !valid {
                return paramErr;
            }
        }
        (
            opened,
            state.mixer,
            state.converters.get(&source).copied(),
            state.unit,
        )
    };
    if opened {
        if let (Some((mixer_node, mixer_unit)), Some(converter)) = (mixer, converter) {
            if destination == mixer_node {
                let result = attach_converter(env, mixer_unit, destination_bus, converter);
                if result != 0 {
                    return result;
                }
            }
        } else if let (Some((mixer_node, _)), Some(output)) = (mixer, output) {
            if source == mixer_node
                && audio_components::State::get(&mut env.framework_state)
                    .audio_component_instances
                    .get(&output)
                    .is_some_and(|unit| unit.render_callback.is_some())
            {
                return paramErr;
            }
        }
    }
    env.framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
        .unwrap()
        .connections
        .push(connection);
    0
}

fn AUGraphDisconnectNodeInput(
    env: &mut Environment,
    graph: AUGraph,
    destination: AUNode,
    destination_bus: u32,
) -> OSStatus {
    let (connection, mixer, converter) = {
        let Some(state) = env
            .framework_state
            .audio_toolbox
            .au_graph
            .graphs
            .get_mut(&graph)
        else {
            return paramErr;
        };
        if !state.nodes.contains_key(&destination) || state.initialized {
            return paramErr;
        }
        let Some(index) = state
            .connections
            .iter()
            .position(|c| c.destination == destination && c.destination_bus == destination_bus)
        else {
            return paramErr;
        };
        let connection = state.connections.remove(index);
        (
            connection,
            state.mixer,
            state.converters.get(&connection.source).copied(),
        )
    };
    if let (Some((mixer_node, mixer)), Some(converter)) = (mixer, converter) {
        if connection.destination == mixer_node {
            let object = audio_components::State::get(&mut env.framework_state)
                .audio_component_instances
                .get_mut(&mixer)
                .unwrap();
            let buses = object.mixer_buses.as_mut().unwrap();
            if buses.get(&destination_bus) == Some(&converter) {
                buses.remove(&destination_bus);
            }
        }
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
    let unit = state
        .converters
        .get(&node)
        .copied()
        .or_else(|| {
            state
                .mixer
                .filter(|(mixer_node, _)| *mixer_node == node)
                .map(|(_, unit)| unit)
        })
        .or(state.unit)
        .unwrap_or_else(MutPtr::null);
    if !out_desc.is_null() {
        env.mem.write(out_desc, description);
    }
    if !out_unit.is_null() {
        env.mem.write(out_unit, unit);
    }
    0
}

fn attach_converter(
    env: &mut Environment,
    mixer: AudioUnit,
    bus: u32,
    converter: AudioUnit,
) -> OSStatus {
    let components = audio_components::State::get(&mut env.framework_state);
    let Some(existing) = components
        .audio_component_instances
        .get(&mixer)
        .and_then(|unit| unit.mixer_buses.as_ref())
        .and_then(|buses| buses.get(&bus))
        .copied()
    else {
        let parent = components.audio_component_instances.get(&mixer).unwrap();
        let (master_gain, master_enabled, spatial) = (
            parent.mixer_master_gain,
            parent.mixer_enabled,
            parent.mixer_subtype == Some(u32::from_be_bytes(*b"3dem")),
        );
        let child = components
            .audio_component_instances
            .get_mut(&converter)
            .unwrap();
        child.mixer_master_gain = master_gain;
        child.mixer_master_enabled = master_enabled;
        child.mixer_voice = true;
        child.mixer_spatial = spatial;
        let object = components
            .audio_component_instances
            .get_mut(&mixer)
            .unwrap();
        object.mixer_buses.as_mut().unwrap().insert(bus, converter);
        object.mixer_bus_count = object.mixer_bus_count.max(bus + 1);
        return 0;
    };
    if existing == converter {
        return 0;
    }
    let Some(old) = components.audio_component_instances.get(&existing) else {
        return paramErr;
    };
    if old.passthrough_converter || old.render_callback.is_some() || old.initialized || old.started
    {
        return paramErr;
    }
    let format = old.input_stream_format;
    let controls = (
        old.mixer_gain,
        old.mixer_master_gain,
        old.mixer_enabled,
        old.mixer_master_enabled,
        old.mixer_voice,
        old.mixer_spatial,
        old.mixer_rate,
        old.mixer_azimuth,
        old.mixer_elevation,
        old.mixer_distance,
    );
    let Some(new) = components.audio_component_instances.get_mut(&converter) else {
        return paramErr;
    };
    if let Some(format) = format {
        if new.input_stream_format.is_some_and(|input| input != format)
            || new
                .output_stream_format
                .is_some_and(|output| output != format)
        {
            return paramErr;
        }
        new.output_stream_format = Some(format);
    }
    (
        new.mixer_gain,
        new.mixer_master_gain,
        new.mixer_enabled,
        new.mixer_master_enabled,
        new.mixer_voice,
        new.mixer_spatial,
        new.mixer_rate,
        new.mixer_azimuth,
        new.mixer_elevation,
        new.mixer_distance,
    ) = controls;
    components
        .audio_component_instances
        .get_mut(&mixer)
        .unwrap()
        .mixer_buses
        .as_mut()
        .unwrap()
        .insert(bus, converter);
    audio_components::AudioComponentInstanceDispose(env, existing);
    0
}

fn analyze_graph(state: &Graph, complete: bool) -> Option<(AUNode, Option<AUNode>, Vec<AUNode>)> {
    let (mut output, mut mixer, mut converters) = (None, None, Vec::new());
    for (&node, desc) in &state.nodes {
        if desc.component_manufacturer != u32::from_be_bytes(*b"appl") {
            return None;
        }
        if desc.component_type == u32::from_be_bytes(*b"auou")
            && desc.component_sub_type == u32::from_be_bytes(*b"rioc")
            && output.is_none()
        {
            output = Some(node);
        } else if desc.component_type == u32::from_be_bytes(*b"aumx")
            && (desc.component_sub_type == u32::from_be_bytes(*b"mcmx")
                || desc.component_sub_type == u32::from_be_bytes(*b"3dem"))
            && mixer.is_none()
        {
            mixer = Some(node);
        } else if desc.component_type == u32::from_be_bytes(*b"aufc")
            && desc.component_sub_type == u32::from_be_bytes(*b"conv")
        {
            converters.push(node);
        } else {
            return None;
        }
    }
    let output = output?;
    if mixer.is_none() && (!converters.is_empty() || !state.connections.is_empty()) {
        return None;
    }
    let mut converter_sources = std::collections::HashSet::new();
    let mut output_connected = false;
    for connection in &state.connections {
        if connection._source_bus != 0 {
            return None;
        }
        if Some(connection.source) == mixer
            && connection.destination == output
            && connection.destination_bus == 0
        {
            if output_connected {
                return None;
            }
            output_connected = true;
        } else if converters.contains(&connection.source)
            && Some(connection.destination) == mixer
            && connection.destination_bus < 256
        {
            if !converter_sources.insert(connection.source) {
                return None;
            }
        } else {
            return None;
        }
    }
    if complete
        && mixer.is_some()
        && (!output_connected || converter_sources.len() != converters.len())
    {
        return None;
    }
    Some((output, mixer, converters))
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
    let Some((_, mixer_node, converters)) = analyze_graph(state, false) else {
        return paramErr;
    };
    let connections = state.connections.clone();
    let callbacks = state.render_notifies.clone();
    let mixer_subtype = mixer_node.map(|node| state.nodes[&node].component_sub_type);
    let unit = audio_components::create_audio_unit_instance(env);
    let mixer = mixer_node.map(|node| {
        let mixer = audio_components::create_audio_unit_instance(env);
        let object = audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&mixer)
            .unwrap();
        object.mixer_buses = Some(HashMap::new());
        object.mixer_subtype = mixer_subtype;
        (node, mixer)
    });
    let mut converter_units = BTreeMap::new();
    for node in converters {
        let converter = audio_components::create_audio_unit_instance(env);
        audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .get_mut(&converter)
            .unwrap()
            .passthrough_converter = true;
        converter_units.insert(node, converter);
    }
    if let Some((mixer_node, mixer_unit)) = mixer {
        for connection in &connections {
            if connection.destination == mixer_node {
                let converter = converter_units[&connection.source];
                if attach_converter(env, mixer_unit, connection.destination_bus, converter) != 0 {
                    unreachable!();
                }
            }
        }
    }
    for (callback, context) in callbacks {
        audio_unit::add_render_notify(env, unit, callback, context, graph.to_bits());
    }
    let state = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
        .unwrap();
    state.unit = Some(unit);
    state.mixer = mixer;
    state.converters = converter_units;
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
    if analyze_graph(state, true).is_none() {
        return paramErr;
    }
    if state.initialized {
        return 0;
    }
    let mixer = state.mixer.map(|(_, unit)| unit);
    if let Some(mixer) = mixer {
        let result = audio_unit::AudioUnitInitialize(env, mixer);
        if result != 0 {
            return result;
        }
    }
    let result = audio_unit::AudioUnitInitialize(env, unit);
    if result != 0 {
        if let Some(mixer) = mixer {
            audio_unit::AudioUnitUninitialize(env, mixer);
        }
        return result;
    }
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
    let mixer = state.mixer.map(|(_, unit)| unit);
    let result = audio_unit::AudioOutputUnitStart(env, unit);
    if result != 0 {
        return result;
    }
    if let Some(mixer) = mixer {
        let result = audio_unit::AudioOutputUnitStart(env, mixer);
        if result != 0 {
            audio_unit::AudioOutputUnitStop(env, unit);
            return result;
        }
    }
    0
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
    let Some(unit) = state.unit else {
        return 0;
    };
    let mixer = state.mixer.map(|(_, unit)| unit);
    if let Some(mixer) = mixer {
        if unit_is_running(env, mixer) {
            let result = audio_unit::AudioOutputUnitStop(env, mixer);
            if result != 0 {
                return result;
            }
        }
    }
    if unit_is_running(env, unit) {
        audio_unit::AudioOutputUnitStop(env, unit)
    } else {
        0
    }
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
    if state.unit.is_some_and(|unit| unit_is_running(env, unit))
        || state
            .mixer
            .is_some_and(|(_, unit)| unit_is_running(env, unit))
    {
        return paramErr;
    }
    if !state.initialized {
        return 0;
    }
    let unit = state.unit.unwrap();
    let mixer = state.mixer.map(|(_, unit)| unit);
    let result = audio_unit::AudioUnitUninitialize(env, unit);
    if result != 0 {
        return result;
    }
    if let Some(mixer) = mixer {
        let result = audio_unit::AudioUnitUninitialize(env, mixer);
        if result != 0 {
            return result;
        }
    }
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
    let state = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
        .unwrap();
    let unit = state.unit.take();
    let mixer = state.mixer.take().map(|(_, unit)| unit);
    let converters = std::mem::take(&mut state.converters);
    if let Some(mixer) = mixer {
        audio_components::AudioComponentInstanceDispose(env, mixer);
    }
    for converter in converters.into_values() {
        if audio_components::State::get(&mut env.framework_state)
            .audio_component_instances
            .contains_key(&converter)
        {
            audio_components::AudioComponentInstanceDispose(env, converter);
        }
    }
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
    let unit = state.unit;
    let running = unit.is_some_and(|unit| unit_is_running(env, unit));
    env.mem.write(out_value, u8::from(running));
    0
}

fn unit_is_running(env: &Environment, unit: AudioUnit) -> bool {
    env.framework_state
        .audio_toolbox
        .audio_components
        .audio_component_instances
        .get(&unit)
        .is_some_and(|state| state.started)
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
    if !state.nodes.contains_key(&node)
        || state
            .connections
            .iter()
            .any(|connection| connection.destination == node && connection.destination_bus == bus)
    {
        return paramErr;
    }
    let unit = if let Some(&converter) = state.converters.get(&node) {
        if bus == 0 {
            Some(converter)
        } else {
            None
        }
    } else if let Some((mixer_node, mixer)) = state.mixer {
        if mixer_node == node {
            Some(mixer)
        } else if bus == 0 {
            state.unit
        } else {
            None
        }
    } else if bus == 0 {
        state.unit
    } else {
        None
    };
    let Some(unit) = unit else {
        return paramErr;
    };
    audio_unit::AudioUnitSetProperty(
        env,
        unit,
        23,
        1,
        bus,
        callback.cast(),
        guest_size_of::<audio_components::AURenderCallbackStruct>(),
    )
}

fn AUGraphUpdate(env: &mut Environment, graph: AUGraph, out_updated: MutPtr<u8>) -> OSStatus {
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get(&graph)
    else {
        return paramErr;
    };
    if state.unit.is_none() {
        return paramErr;
    }
    // Changes to an open graph are currently rejected, so none are pending.
    if !out_updated.is_null() {
        env.mem.write(out_updated, 0);
    }
    0
}

fn AUGraphAddRenderNotify(
    env: &mut Environment,
    graph: AUGraph,
    callback: GuestFunction,
    context: ConstVoidPtr,
) -> OSStatus {
    if callback.to_ptr().is_null() {
        return paramErr;
    }
    let Some(state) = env
        .framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
    else {
        return paramErr;
    };
    let unit = state.unit;
    if let Some(unit) = unit {
        let result = audio_unit::add_render_notify(env, unit, callback, context, graph.to_bits());
        if result != 0 {
            return result;
        }
    }
    env.framework_state
        .audio_toolbox
        .au_graph
        .graphs
        .get_mut(&graph)
        .unwrap()
        .render_notifies
        .push((callback, context));
    0
}

fn AUGraphRemoveRenderNotify(
    env: &mut Environment,
    graph: AUGraph,
    callback: GuestFunction,
    context: ConstVoidPtr,
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
    state.render_notifies.retain(|(registered, refcon)| {
        registered.to_ptr() != callback.to_ptr() || *refcon != context
    });
    let unit = state.unit;
    if let Some(unit) = unit {
        return audio_unit::remove_render_notify(env, unit, callback, context, graph.to_bits());
    }
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
    export_c_func!(AUGraphUpdate(_, _)),
    export_c_func!(AUGraphAddRenderNotify(_, _, _)),
    export_c_func!(AUGraphRemoveRenderNotify(_, _, _)),
    export_c_func!(AUGraphSetNodeInputCallback(_, _, _, _)),
    export_c_func!(AUGraphGetIndNode(_, _, _)),
    export_c_func!(AUGraphConnectNodeInput(_, _, _, _, _)),
    export_c_func!(AUGraphDisconnectNodeInput(_, _, _)),
];
