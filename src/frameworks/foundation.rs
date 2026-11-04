/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The Foundation framework.
//!
//! A concept that Foundation really likes is "class clusters": abstract classes
//! with private concrete implementations. Apple has their own explanation of it
//! in [Cocoa Core Competencies](https://developer.apple.com/library/archive/documentation/General/Conceptual/DevPedia-CocoaCore/ClassCluster.html).
//! Being aware of this concept will make common types like `NSArray` and
//! `NSString` easier to understand.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, MutPtr};
use crate::objc::id;
use crate::Environment;

pub mod _nib_archive_decoder;
pub mod ab_people_picker_navigation_controller;
pub mod asidentifier_manager;
pub mod chipmunk_space;
pub mod ns_array;
pub mod ns_attributed_string;
pub mod ns_autorelease_pool;
pub mod ns_bundle;
pub mod ns_calendar;
pub mod ns_character_set;
pub mod ns_coder;
pub mod ns_condition;
pub mod ns_data;
pub mod ns_date;
pub mod ns_date_formatter;
pub mod ns_decimal_number;
pub mod ns_dictionary;
pub mod ns_enumerator;
pub mod ns_error;
pub mod ns_exception;
pub mod ns_file_handle;
pub mod ns_file_manager;
pub mod ns_garbage_collector;
pub mod ns_hash_table;
pub mod ns_host;
pub mod ns_http_cookie_storage;
pub mod ns_index_path;
pub mod ns_index_set;
pub mod ns_input_stream;
pub mod ns_invocation;
pub mod ns_json_serialization;
pub mod ns_keyed_archiver;
pub mod ns_keyed_unarchiver;
pub mod ns_locale;
pub mod ns_lock;
pub mod ns_log;
pub mod ns_map_table;
pub mod ns_metadata_query;
pub mod ns_method_signature;
pub mod ns_net_service;
pub mod ns_notification;
pub mod ns_notification_center;
pub mod ns_null;
pub mod ns_number_formatter;
pub mod ns_objc_runtime;
pub mod ns_object;
pub mod ns_operation;
pub mod ns_ordered_set;
pub mod ns_persistent_store_coordinator;
pub mod ns_pointer_array;
pub mod ns_port;
pub mod ns_predicate;
pub mod ns_process_info;
pub mod ns_progress;
pub mod ns_property_list_serialization;
pub mod ns_regular_expression;
pub mod ns_run_loop;
pub mod ns_scanner;
pub mod ns_set;
pub mod ns_sort_descriptor;
pub mod ns_string;
pub mod ns_text_checking_result;
pub mod ns_thread;
pub mod ns_time_zone;
pub mod ns_timer;
pub mod ns_ubiquitous_key_value_store;
pub mod ns_undo_manager;
pub mod ns_url;
pub mod ns_url_connection;
pub mod ns_url_request;
pub mod ns_url_response;
pub mod ns_url_session;
pub mod ns_user_defaults;
pub mod ns_uuid;
pub mod ns_value;
pub mod ns_xml_parser;
pub mod url_loading;
pub mod url_protocol;

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/Frameworks/Foundation.framework/Foundation",
    aliases: &[],
    class_exports: &[
        url_protocol::CLASSES,
        ns_url_session::CLASSES,
        ns_url_response::CLASSES,
        ns_undo_manager::CLASSES,
        ns_ubiquitous_key_value_store::CLASSES,
        ns_text_checking_result::CLASSES,
        ns_sort_descriptor::CLASSES,
        ns_regular_expression::CLASSES,
        ns_progress::CLASSES,
        ns_predicate::CLASSES,
        ns_port::CLASSES,
        ns_pointer_array::CLASSES,
        ns_persistent_store_coordinator::CLASSES,
        ns_ordered_set::CLASSES,
        ns_net_service::CLASSES,
        ns_metadata_query::CLASSES,
        ns_map_table::CLASSES,
        ns_input_stream::CLASSES,
        ns_index_set::CLASSES,
        ns_http_cookie_storage::CLASSES,
        ns_host::CLASSES,
        ns_hash_table::CLASSES,
        ns_decimal_number::CLASSES,
        ns_condition::CLASSES,
        chipmunk_space::CLASSES,
        asidentifier_manager::CLASSES,
        ab_people_picker_navigation_controller::CLASSES,
        // Block literals use NSObject methods; keep the private class in a
        // framework linked after libobjc, including in legacy SDK test stubs.
        crate::libc::dispatch::CLASSES,
        _nib_archive_decoder::CLASSES,
        ns_array::CLASSES,
        ns_attributed_string::CLASSES,
        ns_autorelease_pool::CLASSES,
        ns_bundle::CLASSES,
        ns_calendar::CLASSES,
        ns_character_set::CLASSES,
        ns_coder::CLASSES,
        ns_data::CLASSES,
        ns_date::CLASSES,
        ns_date_formatter::CLASSES,
        ns_dictionary::CLASSES,
        ns_enumerator::CLASSES,
        ns_exception::CLASSES,
        ns_error::CLASSES,
        ns_file_handle::CLASSES,
        ns_file_manager::CLASSES,
        ns_garbage_collector::CLASSES,
        ns_index_path::CLASSES,
        ns_invocation::CLASSES,
        ns_json_serialization::CLASSES,
        ns_keyed_archiver::CLASSES,
        ns_keyed_unarchiver::CLASSES,
        ns_locale::CLASSES,
        ns_lock::CLASSES,
        ns_notification::CLASSES,
        ns_notification_center::CLASSES,
        ns_null::CLASSES,
        ns_operation::CLASSES,
        ns_number_formatter::CLASSES,
        ns_method_signature::CLASSES,
        ns_object::CLASSES,
        ns_process_info::CLASSES,
        ns_property_list_serialization::CLASSES,
        ns_run_loop::CLASSES,
        ns_scanner::CLASSES,
        ns_set::CLASSES,
        ns_string::CLASSES,
        ns_thread::CLASSES,
        ns_timer::CLASSES,
        ns_time_zone::CLASSES,
        ns_url::CLASSES,
        ns_url_connection::CLASSES,
        ns_url_request::CLASSES,
        ns_user_defaults::CLASSES,
        ns_value::CLASSES,
        ns_uuid::CLASSES,
        ns_xml_parser::CLASSES,
    ],
    constant_exports: &[
        ns_persistent_store_coordinator::CONSTANTS,
        ns_calendar::CONSTANTS,
        ns_error::CONSTANTS,
        ns_exception::CONSTANTS,
        ns_file_manager::CONSTANTS,
        ns_keyed_unarchiver::CONSTANTS,
        ns_locale::CONSTANTS,
        ns_run_loop::CONSTANTS,
    ],
    function_exports: &[
        FUNCTIONS,
        ns_autorelease_pool::FUNCTIONS,
        ns_exception::FUNCTIONS,
        ns_file_manager::FUNCTIONS,
        ns_log::FUNCTIONS,
        ns_objc_runtime::FUNCTIONS,
        ns_object::FUNCTIONS,
    ],
};

#[derive(Default)]
pub struct State {
    ns_exception: ns_exception::State,
    ns_bundle: ns_bundle::State,
    ns_http_cookie_storage: ns_http_cookie_storage::State,
    pub ns_ubiquitous_key_value_store: ns_ubiquitous_key_value_store::State,
    pub ns_undo_manager: ns_undo_manager::State,
    ns_url_session: ns_url_session::State,
    url_protocol: url_protocol::State,
    ns_file_manager: ns_file_manager::State,
    ns_locale: ns_locale::State,
    ns_notification_center: ns_notification_center::State,
    ns_null: ns_null::State,
    ns_process_info: ns_process_info::State,
    ns_string: ns_string::State,
    ns_thread: ns_thread::State,
    ns_time_zone: ns_time_zone::State,
    ns_user_defaults: ns_user_defaults::State,
    ns_url_connection: ns_url_connection::State,
}

#[derive(Default)]
pub struct ThreadLocalState {
    ns_autorelease_pool: ns_autorelease_pool::ThreadLocalState,
    ns_run_loop: ns_run_loop::ThreadLocalState,
}

pub type NSInteger = i32;
pub type NSUInteger = u32;

// this should be equal to NSIntegerMax
pub const NSNotFound: i32 = 0x7fffffff;

#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct NSRange {
    pub location: NSUInteger,
    pub length: NSUInteger,
}
unsafe impl crate::mem::SafeRead for NSRange {}
crate::abi::impl_GuestRet_for_large_struct!(NSRange);
impl crate::abi::GuestArg for NSRange {
    const REG_COUNT: usize = 2;

    fn from_regs(regs: &[u32]) -> Self {
        NSRange {
            location: crate::abi::GuestArg::from_regs(&regs[0..1]),
            length: crate::abi::GuestArg::from_regs(&regs[1..2]),
        }
    }
    fn to_regs(self, regs: &mut [u32]) {
        self.location.to_regs(&mut regs[0..1]);
        self.length.to_regs(&mut regs[1..2]);
    }
}

fn NSStringFromRange(env: &mut Environment, range: NSRange) -> id {
    let loc = range.location;
    let len = range.length;
    let string = format!("{{{loc}, {len}}}");
    ns_string::from_rust_string(env, string)
}

pub type NSComparisonResult = NSInteger;
pub const NSOrderedAscending: NSComparisonResult = -1;
pub const NSOrderedSame: NSComparisonResult = 0;
pub const NSOrderedDescending: NSComparisonResult = 1;

/// Number of seconds.
pub type NSTimeInterval = f64;

/// Convert an `NSTimeInterval` to a [`std::time::Duration`], or [`None`] if it
/// is NaN, infinite, negative or too large. Apple's implementation silently
/// clamps these cases instead of aborting.
pub fn ns_time_interval_to_duration(ti: NSTimeInterval) -> Option<std::time::Duration> {
    if !ti.is_finite() || ti < 0.0 {
        return None;
    }
    std::time::Duration::try_from_secs_f64(ti).ok()
}

/// Same as [`ns_time_interval_to_duration`] but returns
/// [`std::time::Duration::ZERO`] when the input would otherwise be rejected.
pub fn ns_time_interval_to_duration_or_zero(ti: NSTimeInterval) -> std::time::Duration {
    ns_time_interval_to_duration(ti).unwrap_or(std::time::Duration::ZERO)
}

/// UTF-16 code unit.
#[allow(non_camel_case_types)]
pub type unichar = u16;

/// Utility to help with implementing the `hash` method, which various classes
/// in Foundation have to do.
fn hash_helper<T: std::hash::Hash>(hashable: &T) -> NSUInteger {
    use std::hash::Hasher;

    // Rust documentation says DefaultHasher::new() should always return the
    // same instance, so this should give consistent hashes.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hashable.hash(&mut hasher);
    let hash_u64: u64 = hasher.finish();
    (hash_u64 as u32) ^ ((hash_u64 >> 32) as u32)
}

pub fn NSGetSizeAndAlignment(
    env: &mut Environment,
    type_ptr: ConstPtr<u8>,
    size_out: MutPtr<NSUInteger>,
    align_out: MutPtr<NSUInteger>,
) -> ConstPtr<u8> {
    let (next_ptr, size, align) = parse_objc_type(env, type_ptr);

    if !size_out.is_null() {
        env.mem.write(size_out, size as NSUInteger);
    }
    if !align_out.is_null() {
        env.mem.write(align_out, align as NSUInteger);
    }

    next_ptr
}

fn parse_objc_type(env: &mut Environment, mut ptr: ConstPtr<u8>) -> (ConstPtr<u8>, u32, u32) {
    // Пропускаем модификаторы типа (const, in, out,
    // inout, bycopy, byref, oneway)
    loop {
        let c = env.mem.read(ptr) as char;

        match c {
            'r' | 'n' | 'N' | 'o' | 'O' | 'R' | 'V' => {
                ptr += 1;
            }
            _ => break,
        }
    }

    let c = env.mem.read(ptr) as char;

    ptr += 1;

    match c {
        // Базовые типы
        'c' | 'C' | 'B' => (ptr, 1, 1),
        's' | 'S' => (ptr, 2, 2),
        'i' | 'I' | 'l' | 'L' | 'f' | 'W' => (ptr, 4, 4),
        'q' | 'Q' | 'd' => (ptr, 8, 8),
        'v' => (ptr, 0, 1), // void

        // Указатели, объекты (id), классы (Class),
        // селекторы (SEL), неизвестные указатели
        // (?)
        '*' | '@' | '#' | ':' | '?' => (ptr, 4, 4),

        // Указатель на другой тип: размер
        // всегда 4, но нужно "проглотить" тип, на
        // который он указывает
        '^' => {
            let (next_ptr, _, _) = parse_objc_type(env, ptr);

            (next_ptr, 4, 4)
        }

        // Массивы: [len+type]
        '[' => {
            let mut len = 0;

            loop {
                let c = env.mem.read(ptr) as char;

                if c.is_ascii_digit() {
                    len = len * 10 + c.to_digit(10).unwrap();

                    ptr += 1;
                } else {
                    break;
                }
            }
            let (mut next_ptr, elem_size, elem_align) = parse_objc_type(env, ptr);

            if env.mem.read(next_ptr) as char == ']' {
                next_ptr += 1;
            }
            (next_ptr, len * elem_size, elem_align)
        }

        // Структуры: {name=types}
        '{' => {
            loop {
                let c = env.mem.read(ptr) as char;

                ptr += 1;
                if c == '=' || c == '}' {
                    if c == '}' {
                        return (ptr, 0, 1);
                    } // Opaque
                    break;
                }
            }
            let mut total_size = 0;

            let mut max_align = 1;
            loop {
                let c = env.mem.read(ptr) as char;

                if c == '}' {
                    ptr += 1;

                    break;
                }
                if c == '\0' {
                    break;
                }

                // Пропускаем имена полей (например:
                // "x"f)
                if c == '"' {
                    ptr += 1;

                    loop {
                        let nc = env.mem.read(ptr) as char;
                        ptr += 1;
                        if nc == '"' {
                            break;
                        }
                    }
                } else {
                    let (next_ptr, elem_size, elem_align) = parse_objc_type(env, ptr);

                    ptr = next_ptr;

                    if elem_align > 0 {
                        let rem = total_size % elem_align;

                        if rem != 0 {
                            total_size += elem_align - rem;
                        }
                        if elem_align > max_align {
                            max_align = elem_align;
                        }
                    }
                    total_size += elem_size;
                }
            }
            if max_align > 0 {
                let rem = total_size % max_align;

                if rem != 0 {
                    total_size += max_align - rem;
                }
            }
            (ptr, total_size, max_align)
        }

        // Объединения: (name=types)
        '(' => {
            loop {
                let c = env.mem.read(ptr) as char;

                ptr += 1;
                if c == '=' || c == ')' {
                    if c == ')' {
                        return (ptr, 0, 1);
                    }
                    break;
                }
            }
            let mut max_size = 0;

            let mut max_align = 1;
            loop {
                let c = env.mem.read(ptr) as char;

                if c == ')' {
                    ptr += 1;

                    break;
                }
                if c == '\0' {
                    break;
                }

                if c == '"' {
                    ptr += 1;
                    loop {
                        let nc = env.mem.read(ptr) as char;
                        ptr += 1;
                        if nc == '"' {
                            break;
                        }
                    }
                } else {
                    let (next_ptr, elem_size, elem_align) = parse_objc_type(env, ptr);

                    ptr = next_ptr;
                    if elem_size > max_size {
                        max_size = elem_size;
                    }
                    if elem_align > max_align {
                        max_align = elem_align;
                    }
                }
            }
            (ptr, max_size, max_align)
        }

        // Битовые поля: bNUM
        'b' => {
            let mut bits = 0;

            loop {
                let c = env.mem.read(ptr) as char;

                if c.is_ascii_digit() {
                    bits = bits * 10 + c.to_digit(10).unwrap();

                    ptr += 1;
                } else {
                    break;
                }
            }
            let bytes = bits.div_ceil(8);

            (ptr, bytes, 1)
        }

        _ => (ptr, 0, 1),
    }
}

const FUNCTIONS: FunctionExports = &[
    export_c_func!(NSStringFromRange(_)),
    export_c_func!(NSGetSizeAndAlignment(_, _, _)),
];
