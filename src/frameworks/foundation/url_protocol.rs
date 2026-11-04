/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `NSURLProtocol` (Foundation/CFNetwork URL loading system).
//!
//! Apps like Chrome call `[NSURLProtocol class]` and
//! `+registerClass:` early in startup to hook into URL loading. We don't
//! implement the full URL-loading machinery, but registering a protocol
//! class must succeed (it returns `YES`) so callers proceed normally.
//! Instances also answer the primitive accessors so the class behaves
//! reasonably if the app tries to use one.

use crate::frameworks::foundation::ns_string::get_static_str;
use crate::frameworks::foundation::NSUInteger;
use crate::objc::{
    id, msg, msg_class, msg_super, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};

#[derive(Default)]
pub struct State {
    /// Classes registered via `+registerClass:`, most recently registered
    /// first (matching the real lookup order).
    registered_classes: Vec<id>,
}

#[derive(Default)]
struct NSURLProtocolHostObject {
    /// `NSURLRequest*`
    request: id,
    /// `id<NSURLProtocolClient>`
    client: id,
    /// Cached `NSURLResponse*` once `didReceiveResponse:` is faked.
    response: id,
    started: bool,
    stopped: bool,
}

impl HostObject for NSURLProtocolHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSURLProtocol: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::<NSURLProtocolHostObject>::default();
    env.objc.alloc_object(this, host_object, &mut env.mem)
}

// MARK: Registering protocol classes

+ (bool)registerClass:(id)protocol_class { // Class
    if protocol_class == nil {
        return false;
    }
    if env
        .framework_state
        .foundation
        .url_protocol
        .registered_classes
        .contains(&protocol_class)
    {
        return true;
    }
    retain(env, protocol_class);
    env.framework_state
        .foundation
        .url_protocol
        .registered_classes
        .insert(0, protocol_class);
    log_dbg!(
        "NSURLProtocol registerClass: {:?}",
        env.objc.get_class_name(protocol_class)
    );
    true
}

+ (())unregisterClass:(id)protocol_class { // Class
    let state = &mut env.framework_state.foundation.url_protocol;
    if let Some(pos) = state.registered_classes.iter().position(|&c| c == protocol_class) {
        state.registered_classes.remove(pos);
        release(env, protocol_class);
    }
}

+ (id)registeredClasses {
    // Return an autoreleased NSArray snapshot.
    let count = env
        .framework_state
        .foundation
        .url_protocol
        .registered_classes
        .len();
    let arr: id = msg_class![env; NSMutableArray arrayWithCapacity:(count as NSUInteger)];
    let classes = env
        .framework_state
        .foundation
        .url_protocol
        .registered_classes
        .clone();
    for &class in &classes {
        () = msg![env; arr addObject:class];
    }
    let res: id = msg![env; arr copy];
    () = msg![env; arr release];
    res
}

// MARK: Canonical requests

+ (id)canonicalRequestForRequest:(id)request { // NSURLRequest*
    // The default implementation returns the request unchanged.
    request
}

+ (bool)requestIsCacheEquivalent:(id)a toRequest:(id)b { // NSURLRequest*, NSURLRequest*
    let a_url: id = msg![env; a URL];
    let b_url: id = msg![env; b URL];
    if a_url == nil || b_url == nil {
        return a_url == b_url;
    }
    let a_abs: id = msg![env; a_url absoluteString];
    let b_abs: id = msg![env; b_url absoluteString];
    a_abs != nil && b_abs != nil && msg![env; a_abs isEqualToString:b_abs]
}

// MARK: Instance init

- (id)initWithRequest:(id)request
         cachedResponse:(id)_cached_response
                 client:(id)client { // NSURLRequest*, NSCachedURLResponse*, id<NSURLProtocolClient>
    let this: id = msg_super![env; this init];
    if this == nil {
        return nil;
    }
    {
        let host_object = env.objc.borrow_mut::<NSURLProtocolHostObject>(this);
        host_object.request = request;
        host_object.client = client;
    }
    retain(env, request);
    retain(env, client);
    this
}

- (id)request {
    env.objc.borrow::<NSURLProtocolHostObject>(this).request
}

- (id)client {
    env.objc.borrow::<NSURLProtocolHostObject>(this).client
}

- (id)response {
    env.objc.borrow::<NSURLProtocolHostObject>(this).response
}

// MARK: Loading

- (())startLoading {
    let (client, request) = {
        let host_object = env.objc.borrow::<NSURLProtocolHostObject>(this);
        (host_object.client, host_object.request)
    };
    if client == nil {
        return;
    }
    // We cannot actually load anything. Fake an empty-but-successful
    // response so clients that block on a callback make progress instead
    // of hanging.
    let url: id = if request != nil { msg![env; request URL] } else { nil };
    let _abs: id = if url != nil { msg![env; url absoluteString] } else { nil };
    let scheme: id = if url != nil { msg![env; url scheme] } else { nil };
    let mime: &str = if scheme != nil && {
        let lower: id = msg![env; scheme lowercaseString];
        lower != nil && msg![env; lower isEqualToString:(get_static_str(env, "file"))]
    } {
        "text/html"
    } else {
        "application/octet-stream"
    };
    let expected: crate::frameworks::foundation::NSInteger = 0;
    let response: id = msg_class![env; NSHTTPURLResponse alloc];
    let response: id = msg![env; response
        initWithURL:url
        statusCode:200i32
        HTTPVersion:(get_static_str(env, "HTTP/1.1"))
        headerFields:nil
    ];
    if response == nil {
        // Fallback for builds without NSHTTPURLResponse init support.
        let resp2: id = msg_class![env; NSURLResponse alloc];
        let resp2: id = msg![env; resp2
            initWithURL:url
            MIMEType:(get_static_str(env, mime))
            expectedContentLength:expected
            textEncodingName:nil
        ];
        if resp2 == nil {
            return;
        }
        let host_object = env.objc.borrow_mut::<NSURLProtocolHostObject>(this);
        host_object.response = resp2;
        host_object.started = true;
        () = msg![env; client URLProtocol:this didReceiveResponse:resp2 cacheStoragePolicy:0u32];
        let empty_data: id = msg_class![env; NSData data];
        () = msg![env; client URLProtocol:this didLoadData:empty_data];
        () = msg![env; client URLProtocolDidFinishLoading:this];
        return;
    }
    {
        let host_object = env.objc.borrow_mut::<NSURLProtocolHostObject>(this);
        host_object.response = response;
        host_object.started = true;
    }
    () = msg![env; client URLProtocol:this didReceiveResponse:response cacheStoragePolicy:0u32];
    let empty_data: id = msg_class![env; NSData data];
    () = msg![env; client URLProtocol:this didLoadData:empty_data];
    () = msg![env; client URLProtocolDidFinishLoading:this];
}

- (())stopLoading {
    let host_object = env.objc.borrow_mut::<NSURLProtocolHostObject>(this);
    host_object.stopped = true;
}

@end

};
