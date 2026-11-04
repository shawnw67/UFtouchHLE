/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSNotificationCenter`.

use super::ns_notification::NSNotificationName;
use super::ns_string;
use crate::abi::{CallFromHost, GuestFunction};
use crate::Environment;

use crate::objc::{
    id, msg, msg_class, msg_send, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr, SEL,
};
use std::borrow::Cow;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    default_center: Option<id>,
}

#[derive(Clone)]
struct Observer {
    observer: id,
    selector: SEL,
    object: id,
}

struct NSNotificationCenterHostObject {
    observers: HashMap<Option<Cow<'static, str>>, Vec<Observer>>,
    cf_observers: Vec<CfObserver>,
}
impl HostObject for NSNotificationCenterHostObject {}

/// An observer registered with `CFNotificationCenterAddObserver`. Neither
/// `observer` nor `object` is retained, and delivery calls `callback`.
#[derive(Clone)]
pub struct CfObserver {
    pub observer: id,
    /// `CFNotificationCallback`.
    pub callback: GuestFunction,
    /// Notification name to observe; `None` means any name.
    pub name: Option<Cow<'static, str>>,
    /// Object filter; `nil` means any object.
    pub object: id,
}

/// Register a CF-style observer on `center`. Called from
/// `crate::frameworks::core_foundation::cf_notification_center`.
/// `center` must not be nil (the caller guards against that).
pub fn add_cf_observer(env: &mut Environment, center: id, cf_observer: CfObserver) {
    let host_obj = env
        .objc
        .borrow_mut::<NSNotificationCenterHostObject>(center);
    host_obj.cf_observers.push(cf_observer);
}

/// Remove CF-style observers matching `observer`, `name` and `object` from
/// `center` (see `CFNotificationCenterRemoveObserver`). Per CF semantics
/// nothing was retained at registration time, so there is nothing to
/// release here either.
pub fn remove_cf_observer(
    env: &mut Environment,
    center: id,
    observer: id,
    name: Option<Cow<'static, str>>,
    object: id,
) {
    let host_obj = env
        .objc
        .borrow_mut::<NSNotificationCenterHostObject>(center);
    host_obj.cf_observers.retain(|cf| {
        !(cf.observer == observer && cf.name == name && (object == nil || object == cf.object))
    });
}

/// Remove every CF-style observer registered for `observer` from `center`
/// (see `CFNotificationCenterRemoveEveryObserver`).
pub fn remove_every_cf_observer(env: &mut Environment, center: id, observer: id) {
    let host_obj = env
        .objc
        .borrow_mut::<NSNotificationCenterHostObject>(center);
    host_obj.cf_observers.retain(|cf| cf.observer != observer);
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSNotificationCenter: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(NSNotificationCenterHostObject {
        observers: HashMap::new(),
        cf_observers: Vec::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)defaultCenter {
    if let Some(c) = env.framework_state.foundation.ns_notification_center.default_center {
        c
    } else {
        let new: id = msg![env; this new];
        env.framework_state.foundation.ns_notification_center.default_center = Some(new);
        new
    }
}

- (())dealloc {
    let host_obj = env.objc.borrow_mut::<NSNotificationCenterHostObject>(this);
    let observers = std::mem::take(&mut host_obj.observers);
    for observer in observers.values().flatten() {
        release(env, observer.object);
    }
    env.objc.dealloc_object(this, &mut env.mem);
}

- (())addObserver:(id)observer
         selector:(SEL)selector
             name:(NSNotificationName)name
           object:(id)object {
    let name = if name != nil {
        // Usually a static string, so no real copy will happen
        Some(ns_string::to_rust_string(env, name))
    } else {
        None
    };

    log_dbg!(
        "[(NSNotificationCenter*){:?} addObserver:{:?} selector:{:?} name:{:?} object:{:?}",
        this,
        observer,
        selector,
        name,
        object,
    );

    // When adding an observer, only the object is retained so it doesn't get
    // deallocated before the notification is delivered. Some apps, such as
    // Dungeon Hunter 2, rely on this being the case.
    // The observer is not retained to avoid retain cycles.
    // https://stackoverflow.com/a/36582937
    // While not explicitly stated by the documentation, there's a paragraph
    // that hints at this behavior:
    // "If your app targets iOS 9.0 and later or macOS 10.11 and later, you do
    // not need to unregister an observer that you created with this function.
    // If you forget or are unable to remove an observer, the system cleans up
    // the next time it would have posted to it."
    // https://developer.apple.com/documentation/foundation/notificationcenter/addobserver(_:selector:name:object:)?language=objc
    // Implying that prior to these versions, it's unsafe to not remove an
    // observer. It's been observed that some apps expect and rely on this
    // behavior, such as Marmalade SDK games that use the Movie Player
    // (Pandemonium and COD Zombies, for example).

    retain(env, object);

    let controller_notification = matches!(name.as_deref(), Some("GCControllerDidConnectNotification" | "GCControllerDidDisconnectNotification"))
        && env.framework_state.foundation.ns_notification_center.default_center == Some(this);
    let host_obj = env.objc.borrow_mut::<NSNotificationCenterHostObject>(this);
    host_obj.observers.entry(name).or_default().push(Observer {
        observer,
        selector,
        object,
    });
    if controller_notification {
        crate::frameworks::game_controller::request_input(env);
    }
}

- (())removeObserver:(id)observer {
    msg![env; this removeObserver:observer name:nil object:nil]
}

- (())removeObserver:(id)observer
                name:(NSNotificationName)name
              object:(id)object {
    assert!(observer != nil); // TODO

    let name = if name == nil {
        None
    } else {
        // Usually a static string, so no real copy will happen
        Some(ns_string::to_rust_string(env, name))
    };

    log_dbg!(
        "[(NSNotificationCenter*){:?} removeObserver:{:?} name:{:?} object:{:?}",
        this,
        observer,
        name,
        object,
    );

    // TODO: is this the correct behaviour, can an observer be registered
    // several times?
    let mut removed_observers = Vec::new();

    let host_obj = env.objc.borrow_mut::<NSNotificationCenterHostObject>(this);
    if name.is_some() {
        let Some(observers) = host_obj.observers.get_mut(&name) else {
            return;
        };
        remove_observers_internal(observers, &mut removed_observers, observer, object);
    } else {
        for observers in host_obj.observers.values_mut() {
            remove_observers_internal(observers, &mut removed_observers, observer, object);
        }
    };

    for removed_observer in removed_observers {
        release(env, removed_observer.object);
    }
}

- (())postNotification:(id)notification {
    log_dbg!(
        "[(NSNotificationCenter*){:?} postNotification:{:?}]",
        this,
        notification,
    );

    let cf_name_id: id = msg![env; notification name];
    // Usually a static string, so no real copy will happen
    let name = ns_string::to_rust_string(env, cf_name_id);

    let notification_poster: id = msg![env; notification object];

    log_dbg!("Notification is a {:?} posted by {:?}", name, notification_poster);

    let host_obj = env.objc.borrow_mut::<NSNotificationCenterHostObject>(this);
    let cf_observers: Vec<CfObserver> = host_obj
        .cf_observers
        .iter()
        .filter(|cf| cf.name.as_ref().is_none_or(|cf_name| *cf_name == name))
        .cloned()
        .collect();
    let mut observers = host_obj.observers.get(&Some(name)).cloned().unwrap_or_default();
    if let Some(nameless_observers) = host_obj.observers.get(&None) {
        observers.extend(nameless_observers.iter().cloned());
    }
    for Observer { observer, selector, object } in observers {
        // The object argument is a filter for which notification sources the
        // observer is interested in.
        if object != nil && notification_poster != object {
            continue;
        }

        log_dbg!(
            "Notification {:?} observed, sending {:?} message to {:?}",
            notification,
            selector.as_str(&env.mem),
            observer
        );

        // In some cases, observer could be removed during the
        // processing of the notification, effectively releasing it.
        // (This is happening with Spore Origins)
        // We need to retain it for correctness.
        retain(env, observer);
        // Signature should be `- (void)notification:(NSNotification *)notif`.
        let _: () = msg_send(env, (observer, selector, notification));
        release(env, observer);
    }

    if !cf_observers.is_empty() {
        let user_info: id = msg![env; notification userInfo];
        for CfObserver { observer, callback, name: _, object } in cf_observers {
            if object != nil && notification_poster != object {
                continue;
            }
            let _: () = callback.call_from_host(
                env,
                (this, observer, cf_name_id, notification_poster, user_info),
            );
        }
    }
}
- (())postNotificationName:(NSNotificationName)name
                    object:(id)object {
    msg![env; this postNotificationName:name
                                 object:object
                               userInfo:nil]
}
- (())postNotificationName:(NSNotificationName)name
                    object:(id)object
                  userInfo:(id)user_info { // NSDictionary*
    let notification: id = msg_class![env; NSNotification alloc];
    let notification: id = msg![env; notification initWithName:name
                                                        object:object
                                                      userInfo:user_info];
    let _: () = msg![env; this postNotification:notification];
    release(env, notification);
}

@end

};

/// A helper function to populate `removed_observers` with observers
/// removed from `observers` based on `observer` and `object` criteria.
fn remove_observers_internal(
    observers: &mut Vec<Observer>,
    removed_observers: &mut Vec<Observer>,
    observer: id,
    object: id,
) {
    let mut i = 0;
    while i < observers.len() {
        if observers[i].observer == observer && (object == nil || object == observers[i].object) {
            removed_observers.push(observers.swap_remove(i));
        } else {
            i += 1;
        }
    }
}
