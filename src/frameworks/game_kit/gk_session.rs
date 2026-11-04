/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `GKSession`.
//!
//! This is for local multiplayer over Bluetooth or Wi-Fi. touchHLE has no
//! other devices to talk to, so a session never finds any peers, and
//! connecting or sending data fails.

use crate::frameworks::foundation::{ns_array, ns_string, NSInteger, NSTimeInterval};
use crate::mem::MutPtr;
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, ClassExports, HostObject, NSZonePtr,
};

struct GKSessionHostObject {
    /// `NSString*`
    session_id: id,
    /// `NSString*`
    display_name: id,
    session_mode: NSInteger,
    /// Weak reference
    delegate: id,
    available: bool,
    disconnect_timeout: NSTimeInterval,
}
impl HostObject for GKSessionHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation GKSession: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(GKSessionHostObject {
        session_id: nil,
        display_name: nil,
        session_mode: 0,
        delegate: nil,
        available: false,
        disconnect_timeout: 20.0,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithSessionID:(id)session_id // NSString*
            displayName:(id)display_name // NSString*
            sessionMode:(NSInteger)session_mode {
    log!(
        "[(GKSession*){:?} initWithSessionID:{:?} displayName:{:?} sessionMode:{}]: there are no other devices, this session will never find peers",
        this,
        session_id,
        display_name,
        session_mode,
    );
    let session_id: id = msg![env; session_id copy];
    let display_name: id = msg![env; display_name copy];
    let host_object = env.objc.borrow_mut::<GKSessionHostObject>(this);
    host_object.session_id = session_id;
    host_object.display_name = display_name;
    host_object.session_mode = session_mode;
    this
}

- (())dealloc {
    let &GKSessionHostObject {
        session_id,
        display_name,
        ..
    } = env.objc.borrow(this);
    release(env, session_id);
    release(env, display_name);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)delegate {
    env.objc.borrow::<GKSessionHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<GKSessionHostObject>(this).delegate = delegate;
}

- (bool)isAvailable {
    env.objc.borrow::<GKSessionHostObject>(this).available
}
- (())setAvailable:(bool)available {
    env.objc.borrow_mut::<GKSessionHostObject>(this).available = available;
}

- (NSTimeInterval)disconnectTimeout {
    env.objc.borrow::<GKSessionHostObject>(this).disconnect_timeout
}
- (())setDisconnectTimeout:(NSTimeInterval)timeout {
    env.objc.borrow_mut::<GKSessionHostObject>(this).disconnect_timeout = timeout;
}

- (id)sessionID {
    env.objc.borrow::<GKSessionHostObject>(this).session_id
}
- (id)displayName {
    env.objc.borrow::<GKSessionHostObject>(this).display_name
}
- (NSInteger)sessionMode {
    env.objc.borrow::<GKSessionHostObject>(this).session_mode
}
- (id)peerID {
    ns_string::get_static_str(env, "1")
}

- (id)displayNameForPeer:(id)_peer_id {
    nil
}

- (id)peersWithConnectionState:(NSInteger)_state {
    let peers = ns_array::from_vec(env, Vec::new());
    autorelease(env, peers)
}

- (())setDataReceiveHandler:(id)_handler
                withContext:(MutPtr<u8>)_context {
    // No data will ever be received.
}

- (())connectToPeer:(id)peer_id
        withTimeout:(NSTimeInterval)_timeout {
    log!("TODO: [(GKSession*){:?} connectToPeer:{:?} withTimeout:]: there are no peers", this, peer_id);
}
- (())cancelConnectToPeer:(id)_peer_id {
}

- (bool)acceptConnectionFromPeer:(id)_peer_id
                           error:(MutPtr<id>)error { // NSError**
    if !error.is_null() {
        env.mem.write(error, nil);
    }
    false
}
- (())denyConnectionFromPeer:(id)_peer_id {
}

- (bool)sendData:(id)_data // NSData*
         toPeers:(id)_peers // NSArray*
    withDataMode:(NSInteger)_mode
           error:(MutPtr<id>)error { // NSError**
    if !error.is_null() {
        env.mem.write(error, nil);
    }
    false
}
- (bool)sendDataToAllPeers:(id)_data // NSData*
              withDataMode:(NSInteger)_mode
                     error:(MutPtr<id>)error { // NSError**
    if !error.is_null() {
        env.mem.write(error, nil);
    }
    false
}

- (())disconnectPeerFromAllPeers:(id)_peer_id {
}
- (())disconnectFromAllPeers {
}

@end

@implementation GKPeerPickerController: NSObject
// TODO
@end

@implementation GKSessionDelegate: NSObject
// TODO
@end

};
