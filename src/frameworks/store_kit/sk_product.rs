/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `SKProduct`, `SKProductsRequest` and `SKProductsResponse`.
//!
//! There's no App Store to ask, so a products request is answered with the
//! requested products, so that apps can show their shop normally. Their price
//! is NaN, which `NSNumberFormatter` formats as "unavailable" (see
//! [unavailable_price_text]), since buying them always fails (see
//! `SKPaymentQueue`). (A special NSNumber subclass wouldn't survive apps
//! copying the value into a new NSNumber.)

use crate::frameworks::foundation::{ns_array, ns_string, NSUInteger};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};

struct SKProductsRequestHostObject {
    /// `NSSet<NSString *> *`
    product_identifiers: id,
    /// Weak reference
    delegate: id,
    cancelled: bool,
}
impl HostObject for SKProductsRequestHostObject {}

struct SKProductHostObject {
    /// `NSString*`
    product_identifier: id,
}
impl HostObject for SKProductHostObject {}

/// If `number` is a product's price (NaN), the text to show for it instead:
/// these prices can't be paid. For `NSNumberFormatter`.
pub fn unavailable_price_text(env: &mut crate::Environment, number: id) -> Option<&'static str> {
    let value: f64 = msg![env; number doubleValue];
    if !value.is_nan() {
        return None;
    }
    let languages = crate::window::get_preferred_language_codes(env);
    Some(
        if languages.first().is_some_and(|lang| lang.starts_with("zh")) {
            "不可购买"
        } else {
            "Unavailable"
        },
    )
}

struct SKProductsResponseHostObject {
    /// `NSArray<SKProduct *> *`
    products: id,
    /// `NSArray<NSString *> *`
    invalid_product_identifiers: id,
}
impl HostObject for SKProductsResponseHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SKProduct: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(SKProductHostObject { product_identifier: nil });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    let identifier = env.objc.borrow::<SKProductHostObject>(this).product_identifier;
    release(env, identifier);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)productIdentifier {
    env.objc.borrow::<SKProductHostObject>(this).product_identifier
}

- (id)localizedTitle {
    env.objc.borrow::<SKProductHostObject>(this).product_identifier
}

- (id)localizedDescription {
    env.objc.borrow::<SKProductHostObject>(this).product_identifier
}

- (id)price {
    // Should be an NSDecimalNumber, but an NSNumber works the same for most
    // purposes.
    msg_class![env; NSNumber numberWithDouble:(f64::NAN)]
}

- (id)priceLocale {
    let identifier = ns_string::get_static_str(env, "en_US");
    let locale: id = msg_class![env; NSLocale alloc];
    let locale: id = msg![env; locale initWithLocaleIdentifier:identifier];
    autorelease(env, locale)
}

@end

@implementation SKRequest: NSObject
@end

@implementation SKProductsRequest: SKRequest

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(SKProductsRequestHostObject {
        product_identifiers: nil,
        delegate: nil,
        cancelled: false,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (id)initWithProductIdentifiers:(id)ids { // NSSet *
    let ids: id = msg![env; ids copy];
    env.objc.borrow_mut::<SKProductsRequestHostObject>(this).product_identifiers = ids;
    this
}

- (())dealloc {
    let ids = env.objc.borrow::<SKProductsRequestHostObject>(this).product_identifiers;
    release(env, ids);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)delegate {
    env.objc.borrow::<SKProductsRequestHostObject>(this).delegate
}
- (())setDelegate:(id)delegate {
    env.objc.borrow_mut::<SKProductsRequestHostObject>(this).delegate = delegate;
}

- (())start {
    log!("[(SKProductsRequest*){:?} start]: there's no App Store, answering with unavailable prices", this);
    // The answer comes asynchronously, like it would from the App Store.
    // Keep the request alive until then.
    retain(env, this);
    let sel = env.objc.lookup_selector("_Podroid_respond").unwrap();
    () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0f64];
}

- (())cancel {
    env.objc.borrow_mut::<SKProductsRequestHostObject>(this).cancelled = true;
}

- (())_Podroid_respond {
    let &SKProductsRequestHostObject {
        product_identifiers,
        delegate,
        cancelled,
    } = env.objc.borrow(this);
    if !cancelled && delegate != nil {
        let identifiers: Vec<id> = if product_identifiers != nil {
            let all: id = msg![env; product_identifiers allObjects];
            let count: NSUInteger = msg![env; all count];
            (0..count).map(|i| msg![env; all objectAtIndex:i]).collect()
        } else {
            Vec::new()
        };
        let products: Vec<id> = identifiers
            .into_iter()
            .map(|identifier| {
                let product: id = msg_class![env; SKProduct new];
                let identifier: id = msg![env; identifier copy];
                env.objc.borrow_mut::<SKProductHostObject>(product).product_identifier = identifier;
                product
            })
            .collect();
        let products = ns_array::from_vec(env, products);
        let invalid = ns_array::from_vec(env, Vec::new());
        let response: id = msg_class![env; SKProductsResponse alloc];
        *env.objc.borrow_mut::<SKProductsResponseHostObject>(response) =
            SKProductsResponseHostObject {
                products,
                invalid_product_identifiers: invalid,
            };

        if env.objc.object_has_method_named(&env.mem, delegate, "productsRequest:didReceiveResponse:") {
            () = msg![env; delegate productsRequest:this didReceiveResponse:response];
        }
        release(env, response);
        if env.objc.object_has_method_named(&env.mem, delegate, "requestDidFinish:") {
            () = msg![env; delegate requestDidFinish:this];
        }
    }
    release(env, this);
}

@end

@implementation SKProductsResponse: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(SKProductsResponseHostObject {
        products: nil,
        invalid_product_identifiers: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

- (())dealloc {
    let &SKProductsResponseHostObject {
        products,
        invalid_product_identifiers,
    } = env.objc.borrow(this);
    release(env, products);
    release(env, invalid_product_identifiers);
    env.objc.dealloc_object(this, &mut env.mem)
}

- (id)products {
    env.objc.borrow::<SKProductsResponseHostObject>(this).products
}

- (id)invalidProductIdentifiers {
    env.objc.borrow::<SKProductsResponseHostObject>(this).invalid_product_identifiers
}

@end

};
