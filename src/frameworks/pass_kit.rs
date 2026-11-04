//! PassKit framework: Apple Pay / wallet classes.
//!
//! Real apps (e.g. Alto's Adventure with the "remove ads" IAP flow) link
//! against PassKit for `PKPayment*` classes even when they never actually
//! present a payment sheet on the emulated device. We provide the class
//! objects with real host objects so `+class`, allocation and property
//! reads work; any attempt to actually authorize a payment reports
//! `PKPaymentAuthorizationStatusFailure` through the normal delegate
//! callbacks instead of crashing.
//!
//! C symbols resolved here:
//! - `_OBJC_CLASS_$_PKPaymentToken` / `PKContact` / `PKPaymentSummaryItem` /
//!   `PKPayment` — ObjC classes.
//! - `_PKContactField*` — `NSString` constants (OptionSet raw values in
//!   Swift-bridged headers are plain NSString keys).

use crate::frameworks::foundation::ns_string;
use crate::objc::{id, msg, nil, objc_classes, release, ClassExports, HostObject, NSZonePtr};

#[derive(Default)]
pub(crate) struct PKPaymentHostObject {
    /// Identifier of the backing StoreKit payment, if one was attached.
    pub(crate) identifier: id, // NSString or nil
    #[allow(dead_code)]
    pub(crate) method_type: u32,
    pub(crate) summary_items: Vec<id>, // PKPaymentSummaryItem
}

#[derive(Default)]
pub(crate) struct PKPaymentTokenHostObject {
    pub(crate) payment_method_display_name: id, // NSString or nil
    pub(crate) payment_network: id,             // NSString or nil
    pub(crate) transaction_identifier: id,      // NSString or nil
}

#[derive(Default)]
pub(crate) struct PKPaymentSummaryItemHostObject {
    pub(crate) label: id,  // NSString
    pub(crate) amount: id, // NSDecimalNumber
}

#[derive(Default)]
pub(crate) struct PKContactHostObject {
    pub(crate) name: id,           // NSPersonNameComponents or nil
    pub(crate) email_address: id,  // NSString or nil
    pub(crate) phone_number: id,   // CNPhoneNumber or nil
    pub(crate) postal_address: id, // CNPostalAddress or nil
}

impl HostObject for PKPaymentHostObject {}
impl HostObject for PKPaymentTokenHostObject {}
impl HostObject for PKPaymentSummaryItemHostObject {}
impl HostObject for PKContactHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation PKPayment: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(PKPaymentHostObject {
        identifier: nil,
        method_type: 1, // PKPaymentMethodTypeCredit (default; never used in practice)
        summary_items: Vec::new(),
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
- (())dealloc {
    let (identifier, summary_items) = {
        let host_obj: &mut PKPaymentHostObject = env.objc.borrow_mut(this);
        (
            std::mem::take(&mut host_obj.identifier),
            std::mem::take(&mut host_obj.summary_items),
        )
    };
    release(env, identifier);
    for item in summary_items {
        release(env, item);
    }
    env.objc.dealloc_object(this, &mut env.mem)
}
// Token / billing metadata: an app that reaches this far is asking for a
// real Apple Pay sheet, which the emulator cannot show. Every property
// returns an empty/nil value so the app's "payment unavailable" path runs.
- (id)token {
    nil
}
- (id)billingContact {
    nil
}
- (id)shippingContact {
    nil
}
- (id)shippingMethod {
    nil
}
@end

@implementation PKPaymentToken: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(PKPaymentTokenHostObject {
        payment_method_display_name: nil,
        payment_network: nil,
        transaction_identifier: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
- (())dealloc {
    let (display_name, network, transaction_id) = {
        let host_obj: &mut PKPaymentTokenHostObject = env.objc.borrow_mut(this);
        (
            std::mem::take(&mut host_obj.payment_method_display_name),
            std::mem::take(&mut host_obj.payment_network),
            std::mem::take(&mut host_obj.transaction_identifier),
        )
    };
    release(env, display_name);
    release(env, network);
    release(env, transaction_id);
    env.objc.dealloc_object(this, &mut env.mem)
}
- (id)paymentMethodDisplayName {
    let host_obj: &PKPaymentTokenHostObject = env.objc.borrow(this);
    host_obj.payment_method_display_name
}
- (id)paymentNetwork {
    let host_obj: &PKPaymentTokenHostObject = env.objc.borrow(this);
    host_obj.payment_network
}
- (id)transactionIdentifier {
    let host_obj: &PKPaymentTokenHostObject = env.objc.borrow(this);
    host_obj.transaction_identifier
}
@end

@implementation PKPaymentSummaryItem: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(PKPaymentSummaryItemHostObject {
        label: ns_string::get_static_str(env, ""),
        amount: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
+ (id)summaryItemWithLabel:(id)label amount:(id)amount {
    let item: id = msg![env; this alloc];
    let host_obj: &mut PKPaymentSummaryItemHostObject = env.objc.borrow_mut(item);
    host_obj.label = label;
    host_obj.amount = amount;
    item
}
- (())dealloc {
    let (label, amount) = {
        let host_obj: &mut PKPaymentSummaryItemHostObject = env.objc.borrow_mut(this);
        (
            std::mem::take(&mut host_obj.label),
            std::mem::take(&mut host_obj.amount),
        )
    };
    release(env, label);
    release(env, amount);
    env.objc.dealloc_object(this, &mut env.mem)
}
- (id)label {
    let host_obj: &PKPaymentSummaryItemHostObject = env.objc.borrow(this);
    host_obj.label
}
- (())setLabel:(id)label {
    let old_label = {
        let host_obj: &mut PKPaymentSummaryItemHostObject = env.objc.borrow_mut(this);
        std::mem::replace(&mut host_obj.label, label)
    };
    release(env, old_label);
}
- (id)amount {
    let host_obj: &PKPaymentSummaryItemHostObject = env.objc.borrow(this);
    host_obj.amount
}
- (())setAmount:(id)amount {
    let old_amount = {
        let host_obj: &mut PKPaymentSummaryItemHostObject = env.objc.borrow_mut(this);
        std::mem::replace(&mut host_obj.amount, amount)
    };
    release(env, old_amount);
}
@end

@implementation PKContact: NSObject
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(PKContactHostObject {
        name: nil,
        email_address: nil,
        phone_number: nil,
        postal_address: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
- (())dealloc {
    let (name, email, phone, addr) = {
        let host_obj: &mut PKContactHostObject = env.objc.borrow_mut(this);
        (
            std::mem::take(&mut host_obj.name),
            std::mem::take(&mut host_obj.email_address),
            std::mem::take(&mut host_obj.phone_number),
            std::mem::take(&mut host_obj.postal_address),
        )
    };
    release(env, name);
    release(env, email);
    release(env, phone);
    release(env, addr);
    env.objc.dealloc_object(this, &mut env.mem)
}
- (id)name {
    let host_obj: &PKContactHostObject = env.objc.borrow(this);
    host_obj.name
}
- (())setName:(id)name {
    let old = {
        let host_obj: &mut PKContactHostObject = env.objc.borrow_mut(this);
        std::mem::replace(&mut host_obj.name, name)
    };
    release(env, old);
}
- (id)emailAddress {
    let host_obj: &PKContactHostObject = env.objc.borrow(this);
    host_obj.email_address
}
- (())setEmailAddress:(id)email {
    let old = {
        let host_obj: &mut PKContactHostObject = env.objc.borrow_mut(this);
        std::mem::replace(&mut host_obj.email_address, email)
    };
    release(env, old);
}
- (id)phoneNumber {
    let host_obj: &PKContactHostObject = env.objc.borrow(this);
    host_obj.phone_number
}
- (())setPhoneNumber:(id)phone {
    let old = {
        let host_obj: &mut PKContactHostObject = env.objc.borrow_mut(this);
        std::mem::replace(&mut host_obj.phone_number, phone)
    };
    release(env, old);
}
- (id)postalAddress {
    let host_obj: &PKContactHostObject = env.objc.borrow(this);
    host_obj.postal_address
}
- (())setPostalAddress:(id)addr {
    let old = {
        let host_obj: &mut PKContactHostObject = env.objc.borrow_mut(this);
        std::mem::replace(&mut host_obj.postal_address, addr)
    };
    release(env, old);
}
@end

};

pub const CONSTANTS: &[(&str, crate::dyld::HostConstant)] = &[
    // `PKContactField` OptionSet raw values (Swift-bridged NSString keys).
    (
        "_PKContactFieldEmailAddress",
        crate::dyld::HostConstant::NSString("emailAddress"),
    ),
    (
        "_PKContactFieldName",
        crate::dyld::HostConstant::NSString("name"),
    ),
    (
        "_PKContactFieldPhoneNumber",
        crate::dyld::HostConstant::NSString("phoneNumber"),
    ),
    (
        "_PKContactFieldPhoneticName",
        crate::dyld::HostConstant::NSString("phoneticName"),
    ),
    (
        "_PKContactFieldPostalAddress",
        crate::dyld::HostConstant::NSString("postalAddress"),
    ),
];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/PassKit.framework/PassKit",
    aliases: &["/System/Library/Frameworks/PassKit.framework/Versions/A/PassKit"],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[],
};
