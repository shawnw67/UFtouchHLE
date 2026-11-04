/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIImagePickerController`

use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::foundation::ns_string::to_rust_string;
use crate::frameworks::foundation::NSInteger;
use crate::objc::{id, nil, objc_classes, ClassExports};
use crate::Environment;

type UIImagePickerControllerSourceType = NSInteger;

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

// TODO: should extend UINavigationController, which extends
//       UIViewController.
@implementation UIImagePickerController: UIViewController

+ (bool)isSourceTypeAvailable:(UIImagePickerControllerSourceType)_type {
    // For now, simply claim no sources are available.
    // TODO: support some sources.
    false
}

- (())setDelegate:(id)_delegate {
    // TODO
}

@end

};

/// `UIVideoAtPathIsCompatibleWithSavedPhotosAlbum(NSString*)` — Apple's API
/// to ask whether a movie file can be saved to the Camera Roll. touchHLE
/// has no Photos library, so the honest answer is "no". Returning `false`
/// also matches the documented behaviour for files with unsupported
/// codecs/containers.
fn UIVideoAtPathIsCompatibleWithSavedPhotosAlbum(env: &mut Environment, video_path: id) -> bool {
    let path_str = if video_path == nil {
        "(nil)".to_string()
    } else {
        to_rust_string(env, video_path).into_owned()
    };
    log!(
        "UIVideoAtPathIsCompatibleWithSavedPhotosAlbum({:?}): stubbed, returning false",
        path_str
    );
    false
}

/// `UISaveVideoAtPathToSavedPhotosAlbum(NSString*, id, SEL, void*)` — Apple's
/// API to copy a movie to the Camera Roll. touchHLE has no Photos library
/// so we simply log and do nothing; the optional completion selector is
/// not invoked because the documented contract on real iOS is that the
/// callback signals "saved", not "ignored", and we'd rather have a quiet
/// no-op than spoof a false success.
fn UISaveVideoAtPathToSavedPhotosAlbum(
    env: &mut Environment,
    video_path: id,
    _completion_target: id,
    _completion_selector: crate::objc::SEL,
    _context_info: crate::mem::MutVoidPtr,
) {
    let path_str = if video_path == nil {
        "(nil)".to_string()
    } else {
        to_rust_string(env, video_path).into_owned()
    };
    log!(
        "UISaveVideoAtPathToSavedPhotosAlbum({:?}): stubbed (no Photos library in touchHLE)",
        path_str
    );
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(UIVideoAtPathIsCompatibleWithSavedPhotosAlbum(_)),
    export_c_func!(UISaveVideoAtPathToSavedPhotosAlbum(_, _, _, _)),
];
