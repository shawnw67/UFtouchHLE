/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use super::*;
use std::ptr;

#[test]
fn allocator_bindings_call_functions_not_data_symbols() {
    init_once();
    unsafe {
        for _ in 0..32 {
            let p = xmlMalloc(64);
            assert!(!p.is_null());
            let bytes = std::slice::from_raw_parts_mut(p.cast::<u8>(), 64);
            bytes.fill(0xA5);
            assert!(bytes.iter().all(|&b| b == 0xA5));
            xmlFree(p);
        }
        xmlFree(ptr::null_mut());
    }
}

/// Hill Climb's startup log ends at xmlSaveFile(UserDefault.xml). That
/// host-shim path dumps a doc, copies the buffer and calls xmlFree. The old
/// binding tried to execute the allocator variable at that last step.
#[test]
fn user_defaults_dump_and_free_round_trip() {
    init_once();
    unsafe {
        let doc = xmlNewDoc(b"1.0\0".as_ptr());
        assert!(!doc.is_null());
        let root = xmlNewNode(ptr::null_mut(), b"userDefaultRoot\0".as_ptr());
        assert!(!root.is_null());
        assert!(xmlDocSetRootElement(doc, root).is_null());
        assert!(!xmlNewChild(
            root,
            ptr::null_mut(),
            b"coins\0".as_ptr(),
            b"135\0".as_ptr(),
        )
        .is_null());

        for format in 0..2 {
            let mut buffer = ptr::null_mut();
            let mut size = 0;
            if format == 0 {
                xmlDocDumpMemory(doc, &mut buffer, &mut size);
            } else {
                xmlDocDumpFormatMemory(doc, &mut buffer, &mut size, 1);
            }
            assert!(!buffer.is_null());
            assert!(size > 0);
            let bytes = read_xml_str(buffer);
            assert_eq!(bytes.len(), size as usize);
            xmlFree(buffer.cast());
            assert!(bytes
                .windows(b"<coins>135</coins>".len())
                .any(|w| w == b"<coins>135</coins>"));

            let parsed = xmlReadMemory(
                bytes.as_ptr().cast(),
                bytes.len() as c_int,
                ptr::null(),
                ptr::null(),
                0,
            );
            assert!(!parsed.is_null());
            let content = xmlNodeGetContent(xmlDocGetRootElement(parsed));
            assert!(!content.is_null());
            assert_eq!(
                String::from_utf8(read_xml_str(content)).unwrap().trim(),
                "135"
            );
            xmlFree(content.cast());
            xmlFreeDoc(parsed);
        }
        xmlFreeDoc(doc);
    }
}
