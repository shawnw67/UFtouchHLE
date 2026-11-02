/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIAlertView`.
//!
//! Alerts aren't drawn yet. An alert with a single button is dismissed with
//! that button straight after it's shown, because pressing it is the only
//! thing the user could do. Some apps wait for this before continuing, e.g.
//! Grand Theft Auto: Vice City only plays its intro movies once its "reboot
//! your device" alert is dismissed.

use crate::frameworks::foundation::{ns_string, NSInteger};
use crate::objc::{
    id, impl_HostObject_with_superclass, msg, msg_super, nil, objc_classes, release, retain,
    ClassExports, NSZonePtr,
};
use std::borrow::Cow;

#[derive(Default)]
pub struct UIAlertViewHostObject {
    superclass: super::UIViewHostObject,
    /// Weak reference
    delegate: id,
    button_count: NSInteger,
}
impl_HostObject_with_superclass!(UIAlertViewHostObject);

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIAlertView: UIView

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<UIAlertViewHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithTitle:(id)title
                      message:(id)message
                     delegate:(id)delegate
            cancelButtonTitle:(id)cancelButtonTitle
            otherButtonTitles:(id)otherButtonTitles, ...args {

    log!("TODO: [(UIAlertView*){:?} initWithTitle:{:?} message:{:?} delegate:{:?} cancelButtonTitle:{:?} otherButtonTitles:{:?}]", this, title, message, delegate, cancelButtonTitle, otherButtonTitles);

    let msg = if message == nil { Cow::from("(nil)") } else { ns_string::to_rust_string(env, message) };
    let title = if title == nil { Cow::from("(nil)") } else { ns_string::to_rust_string(env, title) };
    log!("UIAlertView: title: {:?}, message: {:?}", title, msg);
    // MHP2G save-analysis: when the game shows a save-load alert (e.g.
    // "游戏存档已损坏"), dump the captured CPU read/write logs so we can
    // recover which beta stream the game used to decrypt the inner layer.
    if crate::save_research::enabled() {
        crate::cpu::dump_mhp2g_write_log();
        // Also snapshot the main save buffers (plaintext region 0x345dae0 and
        // ciphertext staging 0x31561000) at alert time.
        let grab = |addr: u32| -> &[u8] {
            env.mem
                .get_bytes_fallible(crate::mem::Ptr::from_bits(addr), 0x170000)
                .unwrap_or(&[])
        };
        crate::save_research::write("alert_plain_345dae0.bin", grab(0x345dae0));
        crate::save_research::write("alert_cipher_31561000.bin", grab(0x31561000));
        crate::save_research::write("alert_cipher_313f6000.bin", grab(0x313f6000));
        log!("[MHP2G] alert-time buffer snapshot dumped");
    }

    let mut button_count = 0;
    if cancelButtonTitle != nil {
        button_count += 1;
    }
    if otherButtonTitles != nil {
        button_count += 1;
        let mut varargs = args.start();
        loop {
            let next_title: id = varargs.next(env);
            if next_title == nil {
                break;
            }
            button_count += 1;
        }
    }

    let this: id = msg_super![env; this init];
    let host_object = env.objc.borrow_mut::<UIAlertViewHostObject>(this);
    host_object.delegate = delegate;
    host_object.button_count = button_count;
    this
}

- (id)delegate {
    env.objc.borrow::<UIAlertViewHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<UIAlertViewHostObject>(this).delegate = delegate;
}

- (NSInteger)numberOfButtons {
    env.objc.borrow::<UIAlertViewHostObject>(this).button_count
}

- (NSInteger)addButtonWithTitle:(id)title {
    log!("TODO: [(UIAlertView *){:?} addButtonWithTitle:{}]", this, ns_string::to_rust_string(env, title));
    let host_object = env.objc.borrow_mut::<UIAlertViewHostObject>(this);
    host_object.button_count += 1;
    host_object.button_count - 1
}

- (())show {
    if env.objc.borrow::<UIAlertViewHostObject>(this).button_count != 1 {
        log!("TODO: [(UIAlertView*){:?} show]", this);
        return;
    }
    log!("[(UIAlertView*){:?} show]: alerts aren't drawn yet, pressing its only button", this);
    // The delegate is told asynchronously, like when the user presses it.
    retain(env, this);
    let sel = env.objc.lookup_selector("_Podroid_pressOnlyButton").unwrap();
    () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0f64];
}

- (())_Podroid_pressOnlyButton {
    let delegate = env.objc.borrow::<UIAlertViewHostObject>(this).delegate;
    let index: NSInteger = 0;
    if delegate != nil {
        if env.objc.object_has_method_named(&env.mem, delegate, "alertView:clickedButtonAtIndex:") {
            () = msg![env; delegate alertView:this clickedButtonAtIndex:index];
        }
        if env.objc.object_has_method_named(&env.mem, delegate, "alertView:willDismissWithButtonIndex:") {
            () = msg![env; delegate alertView:this willDismissWithButtonIndex:index];
        }
        if env.objc.object_has_method_named(&env.mem, delegate, "alertView:didDismissWithButtonIndex:") {
            () = msg![env; delegate alertView:this didDismissWithButtonIndex:index];
        }
    }
    release(env, this);
}

- (())setAlertViewStyle:(i32)style {
    // The existing implementation only supports the default button alert.
    assert_eq!(style, 0, "Text-entry alert styles are not implemented");
}

- (i32)alertViewStyle { 0 }

@end

};
