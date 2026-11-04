/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0.
 */
#![allow(dead_code)]
//! `LocalyticsAmpSession` full implementation.

use crate::objc::{id, objc_classes, ClassExports, HostObject, NSZonePtr};

struct LocalyticsAmpSessionHostObject {}
impl HostObject for LocalyticsAmpSessionHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

    // Обязательная строка для макроса в TouchHLE!
    (env, this, _cmd);

    @implementation LocalyticsAmpSession: NSObject
    + (id)allocWithZone:(NSZonePtr)_zone {
        let host_object = Box::new(LocalyticsAmpSessionHostObject {});
        env.objc.alloc_object(this, host_object, &mut env.mem)
    }
    + (())startSession:(id)_app_key {
        log!("LocalyticsAmpSession [class] startSession:");
    }
    + (())tagEvent:(id)_event {
        log!("LocalyticsAmpSession [class] tagEvent:");
    }

        - (id)init {
        this
    }
    - (id)LocalyticsSession:(id)_app_key {
        this
    }
    - (())startSession:(id)_app_key {
        log!("LocalyticsAmpSession startSession: started");
    }
    - (())open {
        log!("LocalyticsAmpSession open: session opened");
    }
    - (())close {
        log!("LocalyticsAmpSession close: session closed");
    }
    - (())upload {
        log!("LocalyticsAmpSession upload: uploading data");
    }
    - (())resume {
        log!("LocalyticsAmpSession resume: session resumed");
    }
    - (())tagEvent:(id)_event {
        log!("LocalyticsAmpSession tagEvent: recorded");
    }
    - (())tagEvent:(id)_event attributes:(id)_attributes {
        log!("LocalyticsAmpSession tagEvent:attributes: recorded");
    }
    - (())tagScreen:(id)_screen {
        log!("LocalyticsAmpSession tagScreen: recorded");
    }
@end
};
