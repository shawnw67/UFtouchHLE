/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
// Allow the crate to have a non-snake-case name (touchHLE).
// This also allows items in the crate to have non-snake-case names.
#![allow(non_snake_case)]

fn main() -> Result<(), String> {
    if std::env::args().nth(1).as_deref() == Some("--multiplayer") {
        Podroid::lan::run_cli(std::env::args().skip(2))
    } else {
        Podroid::main(std::env::args())
    }
}
