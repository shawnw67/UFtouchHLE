/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `UIPasteboard`.

use crate::frameworks::foundation::ns_string;
use crate::objc::{id, nil, objc_classes, release, ClassExports, TrivialHostObject};
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    named: HashMap<String, id>,
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation UIPasteboard: NSObject

+ (id)pasteboardWithName:(id)name create:(bool)create {
    let name = ns_string::to_rust_string(env, name).into_owned();
    if let Some(&board) = env.framework_state.uikit.ui_pasteboard.named.get(&name) {
        return board;
    }
    if !create {
        return nil;
    }
    // Named boards are private to this emulated process for now.
    let board = env.objc.alloc_object(this, Box::new(TrivialHostObject), &mut env.mem);
    env.framework_state.uikit.ui_pasteboard.named.insert(name, board);
    board
}

+ (())removePasteboardWithName:(id)name {
    let name = ns_string::to_rust_string(env, name).into_owned();
    if let Some(board) = env.framework_state.uikit.ui_pasteboard.named.remove(&name) {
        release(env, board);
    }
}

@end

};
