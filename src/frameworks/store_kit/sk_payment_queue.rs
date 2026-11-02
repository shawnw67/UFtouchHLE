/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `SKPaymentQueue`.
//!
//! Payments aren't possible (there's no App Store), so every payment fails,
//! like it would for an app whose in-app purchases no longer exist, and
//! requests to restore purchases are answered with "nothing to restore".

use super::sk_payment::new_failed_transaction;
use crate::abi::{CallFromHost, GuestFunction};
use crate::frameworks::foundation::ns_array;
use crate::mem::ConstPtr;
use crate::objc::{
    autorelease, id, msg, nil, objc_classes, release, retain, ClassExports, HostObject, NSZonePtr,
};
use crate::Environment;

#[derive(Default)]
pub struct State {
    default_queue: Option<id>,
}

/// Inotia 2 deliberately shows nothing when a purchase fails, which is
/// confusing now that purchases always fail. So show the game's own
/// "connection failed" message, the same way it shows its other messages:
/// with its `Msg_SetText` function, which takes the ID of a message in the
/// game's string tables (the strdata*.dat files, starting at entry 3698).
fn inotia2_show_purchase_failed(env: &mut Environment) {
    if env.bundle.bundle_identifier() != "com.com2us.inotia" {
        return;
    }
    /// Address of `Msg_SetText(int string_id, int type)` (Arm code).
    const MSG_SET_TEXT: u32 = 0xe1adc;
    /// Its first instructions, to check this is the expected version.
    const MSG_SET_TEXT_START: [u8; 12] = [
        0x80, 0x40, 0x2d, 0xe9, 0x0d, 0x70, 0xa0, 0xe1, 0x10, 0xd0, 0x4d, 0xe2,
    ];
    /// The message type used for the "connection failed" message.
    const MSG_TYPE: i32 = 3;

    if env
        .mem
        .bytes_at(ConstPtr::<u8>::from_bits(MSG_SET_TEXT), 12)
        != MSG_SET_TEXT_START
    {
        log!("Warning: unexpected version of Inotia 2, not showing a message for the failed purchase");
        return;
    }
    /// ID of the "connection failed" message (the same for all languages).
    const CONNECTION_FAILED: i32 = 158;
    let msg_set_text = GuestFunction::from_addr_with_thumb_bit(MSG_SET_TEXT);
    let (): () = msg_set_text.call_from_host(env, (CONNECTION_FAILED, MSG_TYPE));
}

struct SKPaymentQueueHostObject {
    /// Weak references, like on iPhone OS
    observers: Vec<id>,
    /// `SKPaymentTransaction*`s that haven't been finished yet (strong
    /// references)
    transactions: Vec<id>,
    /// Transactions the observers haven't been told about yet
    undelivered: Vec<id>,
}
impl HostObject for SKPaymentQueueHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SKPaymentQueue: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(SKPaymentQueueHostObject {
        observers: Vec::new(),
        transactions: Vec::new(),
        undelivered: Vec::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

+ (id)defaultQueue {
    if let Some(queue) = env.framework_state.store_kit.sk_payment_queue.default_queue {
        queue
    } else {
        let new: id = msg![env; this new];
        env.framework_state.store_kit.sk_payment_queue.default_queue = Some(new);
        new
    }
}

+ (bool)canMakePayments {
    false
}

- (())addTransactionObserver:(id)observer {
    let observers = &mut env.objc.borrow_mut::<SKPaymentQueueHostObject>(this).observers;
    if !observers.contains(&observer) {
        observers.push(observer);
    }
}

- (())removeTransactionObserver:(id)observer {
    env.objc
        .borrow_mut::<SKPaymentQueueHostObject>(this)
        .observers
        .retain(|&o| o != observer);
}

- (id)transactions {
    let transactions = env.objc.borrow::<SKPaymentQueueHostObject>(this).transactions.clone();
    for &transaction in &transactions {
        retain(env, transaction);
    }
    let array = ns_array::from_vec(env, transactions);
    autorelease(env, array)
}

- (())addPayment:(id)payment { // SKPayment*
    log!("[(SKPaymentQueue*){:?} addPayment:{:?}]: there's no App Store, so the payment fails", this, payment);
    let payment: id = msg![env; payment copy];
    let transaction = new_failed_transaction(env, payment);
    release(env, payment);
    let host_object = env.objc.borrow_mut::<SKPaymentQueueHostObject>(this);
    host_object.transactions.push(transaction);
    host_object.undelivered.push(transaction);
    // Observers are told asynchronously, like on iPhone OS.
    let sel = env.objc.lookup_selector("_Podroid_deliverTransactions").unwrap();
    () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0f64];
}

- (())_Podroid_deliverTransactions {
    let undelivered = std::mem::take(
        &mut env.objc.borrow_mut::<SKPaymentQueueHostObject>(this).undelivered,
    );
    if undelivered.is_empty() {
        return;
    }
    for &transaction in &undelivered {
        retain(env, transaction);
    }
    let array = ns_array::from_vec(env, undelivered);
    let observers = env.objc.borrow::<SKPaymentQueueHostObject>(this).observers.clone();
    for observer in observers {
        if observer != nil
            && env.objc.object_has_method_named(&env.mem, observer, "paymentQueue:updatedTransactions:")
        {
            () = msg![env; observer paymentQueue:this updatedTransactions:array];
        }
    }
    release(env, array);
    inotia2_show_purchase_failed(env);
}

- (())finishTransaction:(id)transaction { // SKPaymentTransaction*
    let host_object = env.objc.borrow_mut::<SKPaymentQueueHostObject>(this);
    let Some(index) = host_object.transactions.iter().position(|&t| t == transaction) else {
        return;
    };
    host_object.transactions.remove(index);
    host_object.undelivered.retain(|&t| t != transaction);
    release(env, transaction);
}

- (())restoreCompletedTransactions {
    // There are no previous purchases to restore.
    let observers = env.objc.borrow::<SKPaymentQueueHostObject>(this).observers.clone();
    for observer in observers {
        if observer != nil
            && env.objc.object_has_method_named(
                &env.mem,
                observer,
                "paymentQueueRestoreCompletedTransactionsFinished:",
            )
        {
            () = msg![env; observer paymentQueueRestoreCompletedTransactionsFinished:this];
        }
    }
}

@end

};
