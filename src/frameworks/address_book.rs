/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! An isolated, in-memory subset of AddressBook.framework.

use crate::dyld::{export_c_func, ConstantExports, FunctionExports, HostConstant, HostDylib};
use crate::frameworks::foundation::ns_array;
use crate::frameworks::foundation::ns_string::{from_rust_string, to_rust_string};
use crate::mem::MutPtr;
use crate::objc::{id, nil, objc_classes, release, retain, ClassExports, HostObject};
use crate::Environment;
use std::collections::HashMap;

#[derive(Default)]
pub struct State {
    next_id: i32,
}
#[derive(Default)]
struct AddressBook {
    records: Vec<id>,
    changed: bool,
}
impl HostObject for AddressBook {}
struct Record {
    record_id: i32,
    kind: u32,
    values: HashMap<i32, id>,
}
impl HostObject for Record {}

pub const CLASSES: ClassExports = objc_classes! {
    (env, this, _cmd);
    @implementation _Podroid_ABAddressBook: NSObject
    - (())dealloc {
        let records = std::mem::take(&mut env.objc.borrow_mut::<AddressBook>(this).records);
        for record in records { release(env, record); }
        env.objc.dealloc_object(this, &mut env.mem)
    }
    @end
    @implementation _Podroid_ABRecord: NSObject
    - (())dealloc {
        let values = std::mem::take(&mut env.objc.borrow_mut::<Record>(this).values);
        for value in values.into_values() { release(env, value); }
        env.objc.dealloc_object(this, &mut env.mem)
    }
    @end
};
fn new_book(env: &mut Environment) -> id {
    let class = env
        .objc
        .get_known_class("_Podroid_ABAddressBook", &mut env.mem);
    env.objc
        .alloc_object(class, Box::<AddressBook>::default(), &mut env.mem)
}
fn new_record(env: &mut Environment, kind: u32) -> id {
    let class = env.objc.get_known_class("_Podroid_ABRecord", &mut env.mem);
    env.objc.alloc_object(
        class,
        Box::new(Record {
            record_id: -1,
            kind,
            values: HashMap::new(),
        }),
        &mut env.mem,
    )
}
fn clear_error(env: &mut Environment, error: MutPtr<id>) {
    if !error.is_null() {
        env.mem.write(error, nil);
    }
}
fn ABAddressBookCreate(env: &mut Environment) -> id {
    new_book(env)
}
fn ABAddressBookCreateWithOptions(env: &mut Environment, _options: id, error: MutPtr<id>) -> id {
    clear_error(env, error);
    new_book(env)
}
fn ABAddressBookGetAuthorizationStatus(_env: &mut Environment) -> i32 {
    3
}
fn ABPersonCreate(env: &mut Environment) -> id {
    new_record(env, 0)
}
fn ABPersonCreateInSource(env: &mut Environment, _source: id) -> id {
    new_record(env, 0)
}
fn ABGroupCreate(env: &mut Environment) -> id {
    new_record(env, 1)
}
fn ABRecordGetRecordID(env: &mut Environment, record: id) -> i32 {
    if record.is_null() {
        -1
    } else {
        env.objc.borrow::<Record>(record).record_id
    }
}
fn ABRecordGetRecordType(env: &mut Environment, record: id) -> u32 {
    if record.is_null() {
        u32::MAX
    } else {
        env.objc.borrow::<Record>(record).kind
    }
}
fn ABRecordCopyValue(env: &mut Environment, record: id, property: i32) -> id {
    if record.is_null() {
        return nil;
    }
    let value = env
        .objc
        .borrow::<Record>(record)
        .values
        .get(&property)
        .copied()
        .unwrap_or(nil);
    if !value.is_null() {
        retain(env, value);
    }
    value
}
fn ABRecordSetValue(
    env: &mut Environment,
    record: id,
    property: i32,
    value: id,
    error: MutPtr<id>,
) -> bool {
    clear_error(env, error);
    if record.is_null() || value.is_null() {
        return false;
    }
    retain(env, value);
    let old = env
        .objc
        .borrow_mut::<Record>(record)
        .values
        .insert(property, value);
    if let Some(old) = old {
        release(env, old);
    }
    true
}
fn ABRecordRemoveValue(
    env: &mut Environment,
    record: id,
    property: i32,
    error: MutPtr<id>,
) -> bool {
    clear_error(env, error);
    if record.is_null() {
        return false;
    }
    let old = env
        .objc
        .borrow_mut::<Record>(record)
        .values
        .remove(&property);
    if let Some(old) = old {
        release(env, old);
    }
    true
}
fn ABRecordCopyCompositeName(env: &mut Environment, record: id) -> id {
    if record.is_null() {
        return nil;
    }
    let object = env.objc.borrow::<Record>(record);
    let first = object.values.get(&0).copied().unwrap_or(nil);
    let last = object.values.get(&1).copied().unwrap_or(nil);
    let first = if first.is_null() {
        String::new()
    } else {
        to_rust_string(env, first).into_owned()
    };
    let last = if last.is_null() {
        String::new()
    } else {
        to_rust_string(env, last).into_owned()
    };
    let name = [first, last]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        nil
    } else {
        from_rust_string(env, name)
    }
}
fn ABAddressBookAddRecord(env: &mut Environment, book: id, record: id, error: MutPtr<id>) -> bool {
    clear_error(env, error);
    if book.is_null() || record.is_null() {
        return false;
    }
    if env
        .objc
        .borrow::<AddressBook>(book)
        .records
        .contains(&record)
    {
        return false;
    }
    if env.objc.borrow::<Record>(record).record_id == -1 {
        let state = &mut env.framework_state.address_book;
        state.next_id = state
            .next_id
            .checked_add(1)
            .expect("address book ID overflow");
        env.objc.borrow_mut::<Record>(record).record_id = state.next_id;
    }
    retain(env, record);
    let book = env.objc.borrow_mut::<AddressBook>(book);
    book.records.push(record);
    book.changed = true;
    true
}
fn ABAddressBookRemoveRecord(
    env: &mut Environment,
    book: id,
    record: id,
    error: MutPtr<id>,
) -> bool {
    clear_error(env, error);
    if book.is_null() || record.is_null() {
        return false;
    }
    let book = env.objc.borrow_mut::<AddressBook>(book);
    let Some(index) = book
        .records
        .iter()
        .position(|&candidate| candidate == record)
    else {
        return false;
    };
    book.records.remove(index);
    book.changed = true;
    release(env, record);
    true
}
fn ABAddressBookHasUnsavedChanges(env: &mut Environment, book: id) -> bool {
    !book.is_null() && env.objc.borrow::<AddressBook>(book).changed
}
fn ABAddressBookSave(env: &mut Environment, _book: id, error: MutPtr<id>) -> bool {
    clear_error(env, error);
    // No persistent store is available yet: do not claim that data was saved.
    false
}
fn ABAddressBookGetPersonCount(env: &mut Environment, book: id) -> i32 {
    if book.is_null() {
        return 0;
    }
    let records = env.objc.borrow::<AddressBook>(book).records.clone();
    records
        .into_iter()
        .filter(|&record| env.objc.borrow::<Record>(record).kind == 0)
        .count() as i32
}
fn ABAddressBookGetPersonWithRecordID(env: &mut Environment, book: id, record_id: i32) -> id {
    if book.is_null() {
        return nil;
    }
    let records = env.objc.borrow::<AddressBook>(book).records.clone();
    records
        .into_iter()
        .find(|&record| {
            let r = env.objc.borrow::<Record>(record);
            r.kind == 0 && r.record_id == record_id
        })
        .unwrap_or(nil)
}
fn ABAddressBookCopyArrayOfAllPeople(env: &mut Environment, book: id) -> id {
    if book.is_null() {
        return nil;
    }
    let records = env.objc.borrow::<AddressBook>(book).records.clone();
    let people: Vec<id> = records
        .into_iter()
        .filter(|&record| env.objc.borrow::<Record>(record).kind == 0)
        .collect();
    for &person in &people {
        retain(env, person);
    }
    ns_array::from_vec(env, people)
}
fn ABAddressBookCopyArrayOfAllPeopleInSource(env: &mut Environment, book: id, _source: id) -> id {
    ABAddressBookCopyArrayOfAllPeople(env, book)
}
fn property_constant(env: &mut Environment, value: i32) -> crate::mem::ConstVoidPtr {
    env.mem.alloc_and_write(value).cast().cast_const()
}
pub const CONSTANTS: ConstantExports = &[
    (
        "_kABPersonFirstNameProperty",
        HostConstant::Custom(|env| property_constant(env, 0)),
    ),
    (
        "_kABPersonLastNameProperty",
        HostConstant::Custom(|env| property_constant(env, 1)),
    ),
    (
        "_kABPersonOrganizationProperty",
        HostConstant::Custom(|env| property_constant(env, 9)),
    ),
    (
        "_kABPersonEmailProperty",
        HostConstant::Custom(|env| property_constant(env, 12)),
    ),
    (
        "_kABPersonPhoneProperty",
        HostConstant::Custom(|env| property_constant(env, 20)),
    ),
    ("_kABHomeLabel", HostConstant::NSString("_$!<Home>!$_")),
    ("_kABWorkLabel", HostConstant::NSString("_$!<Work>!$_")),
    ("_kABOtherLabel", HostConstant::NSString("_$!<Other>!$_")),
];
pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(ABAddressBookCreate()),
    export_c_func!(ABAddressBookCreateWithOptions(_, _)),
    export_c_func!(ABAddressBookGetAuthorizationStatus()),
    export_c_func!(ABAddressBookGetPersonCount(_)),
    export_c_func!(ABAddressBookGetPersonWithRecordID(_, _)),
    export_c_func!(ABAddressBookCopyArrayOfAllPeople(_)),
    export_c_func!(ABAddressBookCopyArrayOfAllPeopleInSource(_, _)),
    export_c_func!(ABAddressBookAddRecord(_, _, _)),
    export_c_func!(ABAddressBookRemoveRecord(_, _, _)),
    export_c_func!(ABAddressBookHasUnsavedChanges(_)),
    export_c_func!(ABAddressBookSave(_, _)),
    export_c_func!(ABPersonCreate()),
    export_c_func!(ABPersonCreateInSource(_)),
    export_c_func!(ABGroupCreate()),
    export_c_func!(ABRecordGetRecordID(_)),
    export_c_func!(ABRecordGetRecordType(_)),
    export_c_func!(ABRecordCopyValue(_, _)),
    export_c_func!(ABRecordCopyCompositeName(_)),
    export_c_func!(ABRecordSetValue(_, _, _, _)),
    export_c_func!(ABRecordRemoveValue(_, _, _)),
];
pub const DYLIB: HostDylib = HostDylib {
    path: "/System/Library/Frameworks/AddressBook.framework/AddressBook",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[CONSTANTS],
    function_exports: &[FUNCTIONS],
};
