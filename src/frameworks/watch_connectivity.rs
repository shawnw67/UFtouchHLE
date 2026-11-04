/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `WatchConnectivity.framework` — the iPhone side of Apple Watch
//! communication (iOS 9 and later).
//!
//! touchHLE has no paired Apple Watch, so what this framework models is
//! precisely what a real iPhone without one does. That distinction
//! matters, because apps use this framework as a feature gate:
//!
//! * `+[WCSession isSupported]` answers `YES` on an iPhone running iOS 9
//!   or later and `NO` on iPad/iPod touch or earlier systems, so apps
//!   that check it before touching the watch APIs take the right branch.
//! * `+[WCSession defaultSession]` returns the process-wide session, or
//!   `nil` when Watch Connectivity is unsupported (as on real iOS).
//! * `-activateSession` succeeds and reports
//!   `WCSessionActivationStateActivated` (2) to the delegate, exactly
//!   like an iPhone with no paired watch.
//! * `-isPaired`, `-isWatchAppInstalled` and `-isReachable` are `NO`, and
//!   anything that would need a counterpart app fails the documented way,
//!   by invoking the app's error handler/out-parameter with a
//!   `WCErrorDomain` error instead of pretending to succeed.
//!
//! Apple's reference:
//! <https://developer.apple.com/documentation/watchconnectivity>

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{ConstantExports, HostConstant, HostDylib};
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSInteger;
use crate::mem::MutPtr;
use crate::objc::{
    id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
    SEL,
};
use crate::Environment;

/// `WCErrorDomain` — the error domain for every error this framework
/// reports with `NSError`.
const WC_ERROR_DOMAIN: &str = "WCErrorDomain";

pub const CONSTANTS: ConstantExports =
    &[("_WCErrorDomain", HostConstant::NSString(WC_ERROR_DOMAIN))];

/// `WCSessionActivationState` (`WatchConnectivity/WCSession.h`):
/// 0 = notActivated, 1 = inactive, 2 = activated. The emulated session
/// only ever moves from notActivated to activated, because nothing can
/// deactivate it (there is no watch to lose).
const ACTIVATION_STATE_NOT_ACTIVATED: NSInteger = 0;
const ACTIVATION_STATE_ACTIVATED: NSInteger = 2;

/// `WCErrorCode` values from `WatchConnectivity/WCError.h`.
const WC_ERROR_SESSION_NOT_ACTIVATED: NSInteger = 7004;
const WC_ERROR_DEVICE_NOT_PAIRED: NSInteger = 7005;
const WC_ERROR_INVALID_PARAMETER: NSInteger = 7008;

/// Per-process WatchConnectivity state.
#[derive(Default)]
pub struct State {
    /// The `+defaultSession` singleton, if it has been created yet.
    default_session: Option<id>,
}

// =============================================================================
// Host objects
// =============================================================================

#[derive(Default)]
struct WCSessionHostObject {
    /// `id<WCSessionDelegate>` — NOT retained (delegates are weak, and a
    /// session outlives its delegate on real iOS).
    delegate: id,
    /// `WCSessionActivationState`.
    activation_state: NSInteger,
    /// `NSDictionary*` last stored by `-updateApplicationContext:error:`
    /// (retained).
    application_context: id,
}
impl HostObject for WCSessionHostObject {}

#[derive(Default)]
struct WCSessionUserInfoTransferHostObject {
    /// `NSDictionary*` being transferred (retained).
    user_info: id,
    transferring: bool,
}
impl HostObject for WCSessionUserInfoTransferHostObject {}

#[derive(Default)]
struct WCSessionFileHostObject {
    /// `NSURL*` (retained).
    file_url: id,
    /// `NSDictionary*` (retained), may be nil.
    metadata: id,
}
impl HostObject for WCSessionFileHostObject {}

#[derive(Default)]
struct WCSessionFileTransferHostObject {
    /// `WCSessionFile*` being transferred (retained).
    file: id,
    transferring: bool,
}
impl HostObject for WCSessionFileTransferHostObject {}

// =============================================================================
// Helpers
// =============================================================================

/// The emulated system version, defaulting to the newest supported one.
fn ios_version(env: &Environment) -> (i32, i32, i32) {
    let mut parts = env
        .options
        .system_version
        .split('.')
        .map(|part| part.parse().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// Whether the emulated device supports Watch Connectivity at all.
///
/// Apple's `+[WCSession isSupported]` is `NO` on iPads (Watch
/// Connectivity requires an iPhone, since only iPhones can pair with an
/// Apple Watch) and on iPods, and the framework does not exist before
/// iOS 9.
fn is_supported(env: &Environment) -> bool {
    let (major, _minor, _patch) = ios_version(env);
    if major < 9 {
        return false;
    }
    match env.options.device_family {
        Some(family) => family != crate::window::DeviceFamily::iPad,
        None => true,
    }
}

/// Build the `NSError` reported for a failed WatchConnectivity operation.
fn session_error(env: &mut Environment, code: NSInteger) -> id {
    let domain = ns_string::get_static_str(env, WC_ERROR_DOMAIN);
    msg_class![env; NSError errorWithDomain:domain code:code userInfo:nil]
}

/// Register a delegate selector and report whether `delegate` implements
/// it. Registering first keeps `msg!`'s selector lookup from aborting on
/// an optional delegate method that no guest class referenced.
fn delegate_responds(env: &mut Environment, delegate: id, name: &str) -> bool {
    if delegate == nil {
        return false;
    }
    let sel: SEL = env
        .objc
        .register_host_selector(name.to_string(), &mut env.mem);
    msg![env; delegate respondsToSelector:sel]
}

/// Call a 32-bit ARM block with one object argument. The invoke function
/// pointer lives at word 3 of the block, and the block itself is the
/// implicit first argument, as elsewhere in touchHLE.
fn invoke_block_with_object(env: &mut Environment, block: id, argument: id) {
    if block == nil {
        return;
    }
    let invoke_ptr: u32 = env.mem.read(block.cast::<u32>() + 3u32);
    if invoke_ptr == 0 {
        return;
    }
    let invoke = GuestFunction::from_addr_with_thumb_bit(invoke_ptr);
    let _: () = invoke.call_from_host(env, (block, argument));
}

/// Report a delivery failure the way a real session without a watch does.
///
/// `argument` is the payload the app tried to send; a nil payload is an
/// invalid parameter rather than a connectivity problem.
fn fail_delivery(env: &mut Environment, this: id, error_handler: id, argument: id) {
    let code = if argument == nil {
        WC_ERROR_INVALID_PARAMETER
    } else if env
        .objc
        .borrow::<WCSessionHostObject>(this)
        .activation_state
        != ACTIVATION_STATE_ACTIVATED
    {
        // Apple requires an activated session for every send/transfer API.
        WC_ERROR_SESSION_NOT_ACTIVATED
    } else {
        // No Apple Watch is paired with the emulated device, so there is
        // no counterpart app to reach.
        WC_ERROR_DEVICE_NOT_PAIRED
    };
    let error = session_error(env, code);
    invoke_block_with_object(env, error_handler, error);
}

// =============================================================================
// Classes
// =============================================================================

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation WCSession: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = WCSessionHostObject {
        delegate: nil,
        activation_state: ACTIVATION_STATE_NOT_ACTIVATED,
        application_context: nil,
    };
    env.objc.alloc_object(this, Box::new(host_object), &mut env.mem)
}

// `+ (BOOL)isSupported;`
//
// The gate every app checks before using this framework. See
// `is_supported()` for the emulated device's rules.
+ (bool)isSupported {
    is_supported(env)
}

// `+ (WCSession *)defaultSession;`
//
// Apple documents this as returning nil when the device does not support
// Watch Connectivity; the session is a process-wide singleton otherwise.
+ (id)defaultSession {
    if !is_supported(env) {
        log_dbg!("[WCSession defaultSession] => nil (not supported)");
        return nil;
    }
    if let Some(session) = env.framework_state.watch_connectivity.default_session {
        return session;
    }
    let session: id = msg![env; this new];
    // The singleton must survive autorelease pool drains; it is never
    // released, like other framework singletons in touchHLE.
    retain(env, session);
    env.framework_state.watch_connectivity.default_session = Some(session);
    session
}

// `- (id<WCSessionDelegate>)delegate;`
- (id)delegate {
    env.objc.borrow::<WCSessionHostObject>(this).delegate
}

// `- (void)setDelegate:(id<WCSessionDelegate>)delegate;`
//
// The delegate property is weak, so it is deliberately not retained.
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<WCSessionHostObject>(this).delegate = delegate;
}

// `- (void)activateSession;`
//
// On an iPhone that has no paired Apple Watch, activation still
// succeeds: the session reports itself as activated but never paired,
// installed or reachable. The delegate is told about the new state, via
// the iOS 9.3+ callback when it implements one and the original
// `sessionDidBecomeActive:` otherwise.
- (())activateSession {
    let delegate = env.objc.borrow::<WCSessionHostObject>(this).delegate;
    let state = env.objc.borrow::<WCSessionHostObject>(this).activation_state;
    if state == ACTIVATION_STATE_ACTIVATED {
        log_dbg!("[WCSession activateSession] already activated");
        return;
    }
    env.objc
        .borrow_mut::<WCSessionHostObject>(this)
        .activation_state = ACTIVATION_STATE_ACTIVATED;
    log!(
        "WCSession activated. No Apple Watch is paired with this device, \
         so the session reports pairing=False, watchAppInstalled=False and \
         reachable=False."
    );
    if delegate == nil {
        return;
    }
    if delegate_responds(env, delegate, "session:activationDidCompleteWithState:error:") {
        let _: () = msg![env; delegate session:this
                             activationDidCompleteWithState:ACTIVATION_STATE_ACTIVATED
                                                   error:nil];
    } else if delegate_responds(env, delegate, "sessionDidBecomeActive:") {
        let _: () = msg![env; delegate sessionDidBecomeActive:this];
    }
}

// `- (WCSessionActivationState)activationState;`
- (NSInteger)activationState {
    env.objc.borrow::<WCSessionHostObject>(this).activation_state
}

// `- (BOOL)isPaired;` — there is never a paired Apple Watch.
- (bool)isPaired {
    false
}

// `- (BOOL)isWatchAppInstalled;` — no watch means no watch app.
- (bool)isWatchAppInstalled {
    false
}

// `- (BOOL)isCompanionAppInstalled;` (iOS 9.3+).
- (bool)isCompanionAppInstalled {
    false
}

// `- (BOOL)isComplicationEnabled;`
- (bool)isComplicationEnabled {
    false
}

// `- (BOOL)isReachable;` — the counterpart app can never be reached.
- (bool)isReachable {
    false
}

// `- (BOOL)iOSDeviceNeedsUnlockAfterRebootForReachability;`
- (bool)iOSDeviceNeedsUnlockAfterRebootForReachability {
    false
}

// `- (NSURL *)watchDirectoryURL;`
- (id)watchDirectoryURL {
    nil
}

// `- (NSDictionary *)applicationContext;`
//
// Apple documents this as an empty dictionary before an application
// context has been set.
- (id)applicationContext {
    let context = env.objc.borrow::<WCSessionHostObject>(this).application_context;
    if context == nil {
        msg_class![env; NSDictionary dictionary]
    } else {
        context
    }
}

// `- (BOOL)updateApplicationContext:(NSDictionary *)applicationContext
//                             error:(NSError **)error;`
//
// Nothing can be delivered without a counterpart app, so the documented
// error is reported and the previous context is kept.
- (bool)updateApplicationContext:(id)application_context error:(MutPtr<id>)error {
    let code = if application_context == nil {
        WC_ERROR_INVALID_PARAMETER
    } else if env
        .objc
        .borrow::<WCSessionHostObject>(this)
        .activation_state
        != ACTIVATION_STATE_ACTIVATED
    {
        WC_ERROR_SESSION_NOT_ACTIVATED
    } else {
        WC_ERROR_DEVICE_NOT_PAIRED
    };
    if !error.is_null() {
        let error_obj = session_error(env, code);
        env.mem.write(error, error_obj);
    }
    false
}

// `- (NSDictionary *)receivedApplicationContext;`
//
// Only the counterpart app can supply this; with no watch it stays empty.
- (id)receivedApplicationContext {
    msg_class![env; NSDictionary dictionary]
}

// `- (void)sendMessage:(NSDictionary *)message
//          replyHandler:(void (^)(NSDictionary *replyMessage))replyHandler
//          errorHandler:(void (^)(NSError *error))errorHandler;`
- (())sendMessage:(id)message
     replyHandler:(id)_reply_handler
     errorHandler:(id)error_handler {
    if message == nil {
        log!("Warning: [WCSession sendMessage:] called with a nil message; \
              reporting WCErrorCodeInvalidParameter.");
    }
    fail_delivery(env, this, error_handler, message);
}

// `- (void)sendMessageData:(NSData *)data
//             replyHandler:(void (^)(NSData *replyData))replyHandler
//             errorHandler:(void (^)(NSError *error))errorHandler;`
- (())sendMessageData:(id)data
         replyHandler:(id)_reply_handler
         errorHandler:(id)error_handler {
    fail_delivery(env, this, error_handler, data);
}

// `- (WCSessionUserInfoTransfer *)transferUserInfo:(NSDictionary *)userInfo;`
//
// Apple documents that this returns nil when the transfer cannot be
// queued; with no paired watch there is nowhere to queue it.
- (id)transferUserInfo:(id)_user_info {
    log_dbg!("[WCSession transferUserInfo:] => nil (no paired watch)");
    nil
}

// `- (WCSessionUserInfoTransfer *)transferCurrentComplicationUserInfo:
//                                        (NSDictionary *)userInfo;`
- (id)transferCurrentComplicationUserInfo:(id)_user_info {
    log_dbg!("[WCSession transferCurrentComplicationUserInfo:] => nil \
              (no paired watch)");
    nil
}

// `- (NSArray<WCSessionUserInfoTransfer *> *)outstandingUserInfoTransfers;`
- (id)outstandingUserInfoTransfers {
    msg_class![env; NSArray array]
}

// `- (WCSessionFileTransfer *)transferFile:(NSURL *)file
//                                 metadata:(NSDictionary *)metadata;`
- (id)transferFile:(id)_file metadata:(id)_metadata {
    log_dbg!("[WCSession transferFile:metadata:] => nil (no paired watch)");
    nil
}

// `- (NSArray<WCSessionFileTransfer *> *)outstandingFileTransfers;`
- (id)outstandingFileTransfers {
    msg_class![env; NSArray array]
}

- (())dealloc {
    let host = env.objc.borrow::<WCSessionHostObject>(this);
    let application_context = host.application_context;
    if application_context != nil {
        release(env, application_context);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation WCSessionUserInfoTransfer: NSObject

// Transfers are never created (there is no watch to send them to), but
// the class exists so that apps and code paths that mention it behave.
+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<WCSessionUserInfoTransferHostObject>::default(),
        &mut env.mem,
    )
}

- (id)userInfo {
    env.objc
        .borrow::<WCSessionUserInfoTransferHostObject>(this)
        .user_info
}

- (bool)isTransferring {
    env.objc
        .borrow::<WCSessionUserInfoTransferHostObject>(this)
        .transferring
}

- (())cancel {
    // Cancelling a non-existent transfer is a no-op.
}

- (())dealloc {
    let user_info = env
        .objc
        .borrow::<WCSessionUserInfoTransferHostObject>(this)
        .user_info;
    if user_info != nil {
        release(env, user_info);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation WCSessionFileTransfer: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(
        this,
        Box::<WCSessionFileTransferHostObject>::default(),
        &mut env.mem,
    )
}

- (id)file {
    env.objc.borrow::<WCSessionFileTransferHostObject>(this).file
}

- (bool)isTransferring {
    env.objc
        .borrow::<WCSessionFileTransferHostObject>(this)
        .transferring
}

- (())cancel {
    // Cancelling a non-existent transfer is a no-op.
}

- (())dealloc {
    let file = env.objc.borrow::<WCSessionFileTransferHostObject>(this).file;
    if file != nil {
        release(env, file);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

@implementation WCSessionFile: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    env.objc.alloc_object(this, Box::<WCSessionFileHostObject>::default(), &mut env.mem)
}

- (id)fileURL {
    env.objc.borrow::<WCSessionFileHostObject>(this).file_url
}

- (id)metadata {
    env.objc.borrow::<WCSessionFileHostObject>(this).metadata
}

- (())dealloc {
    let host = env.objc.borrow::<WCSessionFileHostObject>(this);
    let file_url = host.file_url;
    let metadata = host.metadata;
    if file_url != nil {
        release(env, file_url);
    }
    if metadata != nil {
        release(env, metadata);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}

@end

};

pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/WatchConnectivity.framework/WatchConnectivity",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};
