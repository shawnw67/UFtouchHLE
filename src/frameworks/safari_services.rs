//! SafariServices framework: `SFSafariViewController` and friends.
//!
//! Games import SafariServices for "open terms of service / open sponsor
//! page in-app" flows. On the emulator there is no embedded Safari, so the
//! view controller presents an empty full-screen view and immediately
//! reports completion through the delegate.

use crate::objc::{id, msg, nil, objc_classes, release, ClassExports, HostObject, NSZonePtr};

#[derive(Default)]
pub(crate) struct SFSafariViewControllerHostObject {
    /// NSURL that was passed to the initializer (or nil).
    pub(crate) url: id,
    pub(crate) delegate: id,
}
impl HostObject for SFSafariViewControllerHostObject {}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation SFSafariViewController: UIViewController
+ (id)allocWithZone:(NSZonePtr)_zone {
    let host_object = Box::new(SFSafariViewControllerHostObject {
        url: nil,
        delegate: nil,
    });
    env.objc.alloc_object(this, host_object, &mut env.mem)
}
- (id)initWithURL:(id)url enterReaderIfAvailable:(bool)_enter_reader {
    let host_obj: &mut SFSafariViewControllerHostObject = env.objc.borrow_mut(this);
    host_obj.url = url;
    this
}
- (id)initWithConfiguration:(id)_configuration {
    this
}
- (())dealloc {
    let (url, delegate) = {
        let host_obj: &mut SFSafariViewControllerHostObject = env.objc.borrow_mut(this);
        (host_obj.url, host_obj.delegate)
    };
    release(env, url);
    release(env, delegate);
    env.objc.dealloc_object(this, &mut env.mem)
}
- (())setDelegate:(id)delegate {
    let old = {
        let host_obj: &mut SFSafariViewControllerHostObject = env.objc.borrow_mut(this);
        std::mem::replace(&mut host_obj.delegate, delegate)
    };
    release(env, old);
}
- (id)delegate {
    let host_obj: &SFSafariViewControllerHostObject = env.objc.borrow(this);
    host_obj.delegate
}
- (())viewWillAppear:(bool)_animated {
    // No real Safari: dismiss immediately. The delegate's
    // `safariViewControllerDidFinish:` is invoked first so the app's
    // completion path runs.
    let host_obj: &SFSafariViewControllerHostObject = env.objc.borrow(this);
    let delegate = host_obj.delegate;
    if delegate != nil {
        let _: () = msg![env; delegate safariViewControllerDidFinish:this];
    }
    let _: () = msg![env; this dismissViewControllerAnimated:false completion:nil];
}
@end

};

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/SafariServices.framework/SafariServices",
    aliases: &["/System/Library/Frameworks/SafariServices.framework/Versions/A/SafariServices"],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};
