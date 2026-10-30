/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Darwin libuuid value functions, adapted from HyperHLE-Reborn ff8eb827.
//! Uses UF's uuid dependency; uuid_generate_time is not aliased to version 4.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, MutPtr};
use crate::Environment;
use uuid::Uuid;

fn read(env: &Environment, value: ConstPtr<u8>) -> [u8; 16] {
    env.mem.bytes_at(value, 16).try_into().unwrap()
}
fn write(env: &mut Environment, value: MutPtr<u8>, bytes: [u8; 16]) {
    env.mem.bytes_at_mut(value, 16).copy_from_slice(&bytes);
}
fn uuid_clear(env: &mut Environment, out: MutPtr<u8>) {
    write(env, out, [0; 16]);
}
fn uuid_copy(env: &mut Environment, out: MutPtr<u8>, source: ConstPtr<u8>) {
    let bytes = read(env, source);
    write(env, out, bytes);
}
fn uuid_compare(env: &mut Environment, a: ConstPtr<u8>, b: ConstPtr<u8>) -> i32 {
    match read(env, a).cmp(&read(env, b)) {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}
fn uuid_is_null(env: &mut Environment, value: ConstPtr<u8>) -> i32 {
    i32::from(read(env, value) == [0; 16])
}
fn uuid_generate_random(env: &mut Environment, out: MutPtr<u8>) {
    write(env, out, Uuid::new_v4().into_bytes());
}
fn uuid_generate(env: &mut Environment, out: MutPtr<u8>) {
    uuid_generate_random(env, out);
}
fn uuid_parse(env: &mut Environment, input: ConstPtr<u8>, out: MutPtr<u8>) -> i32 {
    // The C interface requires the canonical 36-byte form. Do not silently
    // accept arbitrary hyphens/braces or discard non-hex characters.
    let bytes = env.mem.cstr_at(input);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return -1;
    };
    if bytes.len() != 36 || [8, 13, 18, 23].iter().any(|i| bytes[*i] != b'-') {
        return -1;
    }
    let Ok(uuid) = Uuid::parse_str(text) else {
        return -1;
    };
    write(env, out, uuid.into_bytes());
    0
}
fn unparse(env: &mut Environment, input: ConstPtr<u8>, out: MutPtr<u8>, upper: bool) {
    let value = Uuid::from_bytes(read(env, input));
    let mut buffer = [0; 37];
    if upper {
        value.hyphenated().encode_upper(&mut buffer);
    } else {
        value.hyphenated().encode_lower(&mut buffer);
    }
    env.mem.bytes_at_mut(out, 37).copy_from_slice(&buffer);
}
fn uuid_unparse(env: &mut Environment, input: ConstPtr<u8>, out: MutPtr<u8>) {
    unparse(env, input, out, true);
}
fn uuid_unparse_upper(env: &mut Environment, input: ConstPtr<u8>, out: MutPtr<u8>) {
    unparse(env, input, out, true);
}
fn uuid_unparse_lower(env: &mut Environment, input: ConstPtr<u8>, out: MutPtr<u8>) {
    unparse(env, input, out, false);
}
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(uuid_clear(_)),
    export_c_func!(uuid_copy(_, _)),
    export_c_func!(uuid_compare(_, _)),
    export_c_func!(uuid_is_null(_)),
    export_c_func!(uuid_generate(_)),
    export_c_func!(uuid_generate_random(_)),
    export_c_func!(uuid_parse(_, _)),
    export_c_func!(uuid_unparse(_, _)),
    export_c_func!(uuid_unparse_lower(_, _)),
    export_c_func!(uuid_unparse_upper(_, _)),
];
