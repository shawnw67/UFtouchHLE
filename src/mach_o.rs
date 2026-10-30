/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Reading of Mach-O files, the executable and library format on iPhone OS.
//!
//! Implemented using the mach_object crate. All usage of that crate should be
//! confined to this module. The goal is to read the Mach-O binary exactly once,
//! storing any information we'll need later.
//!
//! Useful resources:
//! - Apple's [Overview of the Mach-O Executable Format](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/CodeFootprint/Articles/MachOOverview.html) explains what "segments" and "sections" are, and provides short descriptions of the purposes of some common sections.
//! - Apple's old "OS X ABI Mach-O File Format Reference", which is mirrored in [various](https://github.com/aidansteele/osx-abi-macho-file-format-reference) [places](https://www.symbolcrash.com/wp-content/uploads/2019/02/ABI_MachOFormat.pdf) online.
//! - Alex Drummond's [Inside a Hello World executable on OS X](https://adrummond.net/posts/macho) is about macOS circa 2017 rather than iPhone OS circa 2008, so not all of what it says applies, but the sections up to and including "9. The indirect symbol table" are helpful.
//! - The LLVM functions [`RuntimeDyldMachO::populateIndirectSymbolPointersSection`](https://github.com/llvm/llvm-project/blob/2e999b7dd1934a44d38c3a753460f1e5a217e9a5/llvm/lib/ExecutionEngine/RuntimeDyld/RuntimeDyldMachO.cpp#L179-L220) and [`MachOObjectFile::getIndirectSymbolTableEntry`](https://github.com/llvm/llvm-project/blob/3c09ed006ab35dd8faac03311b14f0857b01949c/llvm/lib/Object/MachOObjectFile.cpp#L4803-L4808) are references for how to read the indirect symbol table.
//! - `/usr/include/mach-o/reloc.h` in the macOS SDK was the reference for the
//!   format of relocation entries.
//! - The [source code of the mach_object crate](https://docs.rs/mach_object/latest/src/mach_object/commands.rs.html) has useful comments that don't show up in the generated documentation, e.g. around `DySymTab`.

use crate::abi::GuestFunction;
use crate::fs::{Fs, GuestPath};
use crate::mem::{GuestUSize, Mem, Ptr};
use mach_object::{
    cpu_subtype_t, vm_prot_t, Bind, BindSymbolType, DyLib, LoadCommand, MachCommand, OFile, Rebase,
    Symbol, SymbolIter, ThreadState, N_ARM_THUMB_DEF, S_LAZY_SYMBOL_POINTERS,
    S_MOD_INIT_FUNC_POINTERS, S_NON_LAZY_SYMBOL_POINTERS, S_SYMBOL_STUBS,
};
use std::collections::HashMap;
use std::io::{Cursor, Seek, SeekFrom};

const VM_PROT_READ: vm_prot_t = 1;
const VM_PROT_WRITE: vm_prot_t = 2;
#[allow(dead_code)]
const VM_PROT_EXECUTE: vm_prot_t = 4;

#[derive(Debug)]
pub struct MachO {
    /// Name (for debugging purposes and sorting)
    pub name: String,
    /// Paths of dynamic libraries referenced by the binary.
    pub dynamic_libraries: Vec<String>,
    /// Metadata related to sections.
    pub sections: Vec<Section>,
    /// All defined symbols, including image-local names. Thumb functions
    /// retain their Thumb bit. Do not use this map for cross-image lookup.
    pub exported_symbols: HashMap<String, u32>,
    /// Only symbols marked N_EXT, for resolution from dependent images.
    pub external_symbols: HashMap<String, u32>,
    /// List of addresses and names of external relocations for the dynamic
    /// linker to resolve.
    pub external_relocations: Vec<(u32, String)>,
    /// Address/program counter value for the entry point.
    pub entry_point_pc: Option<u32>,
    /// End address of the highest-addressed segment.
    /// This is used by get_end() to return the first address after the last
    /// segment in the executable.
    pub last_segment_end: u32,
}

#[derive(Debug)]
pub struct Section {
    /// Section name.
    pub name: String,
    /// Section address in memory.
    pub addr: u32,
    /// Section size in bytes.
    pub size: u32,
    /// What type of section is this?
    pub type_: SectionType,
    /// Information specific to special dynamic linker sections, if this is one.
    pub dyld_indirect_symbol_info: Option<DyldIndirectSymbolInfo>,
}

impl Section {
    /// Returns address of the next section.
    pub fn next_section_addr(&self) -> u32 {
        self.addr + self.size
    }
}

/// Various kinds of sections. We're only interested in the ones used by the
/// dynamic linker for now (see also [DyldIndirectSymbolInfo]).
#[derive(Debug, PartialEq, Eq)]
pub enum SectionType {
    /// Normal section, or some section type we don't care about.
    Normal,
    /// Symbol stub section, usually called `__symbol_stub4` or
    /// `__picsymbolstub4`.
    SymbolStubs,
    /// Lazy symbol pointer section, usually called `__la_symbol_ptr`.
    LazySymbolPointers,
    /// Non-lazy symbol pointer section, usually called `__nl_symbol_ptr`.
    NonLazySymbolPointers,
    /// Initialization function pointer section, usually called
    /// `__mod_init_func`.
    ModInitFuncPointers,
}

/// Information relevant to certain special sections which contain a series of
/// pointers or stub functions for indirectly referencing dynamically-linked
/// symbols (see also [SectionType]).
#[derive(Debug)]
pub struct DyldIndirectSymbolInfo {
    /// The size in bytes of an entry (pointer or stub function) in the section.
    pub entry_size: u32,
    /// A list of symbol names corresponding to the entries.
    pub indirect_undef_symbols: Vec<Option<String>>,
}

/// Helper trait that makes [MachO::get_section] work. Yes, this is overkill. :)
pub trait SectionPredicate {
    fn test(&self, section: &Section) -> bool;
}
impl SectionPredicate for &str {
    fn test(&self, section: &Section) -> bool {
        section.name == *self
    }
}
impl SectionPredicate for SectionType {
    fn test(&self, section: &Section) -> bool {
        section.type_ == *self
    }
}

fn get_sym_by_idx<'a>(
    idx: u32,
    (symoff, nsyms, stroff, strsize): (u32, u32, u32, u32),
    is_bigend: bool,
    is_64bit: bool,
    cursor: &'a mut Cursor<&'a [u8]>,
) -> Option<mach_object::Symbol<'a>> {
    if idx >= nsyms {
        return None;
    }

    let symoff = (symoff + idx * 12) as u64;

    cursor.seek(SeekFrom::Start(symoff)).unwrap();

    // This is not how you're supposed to use SymbolIter but the parse_symbol()
    // method on it requires the bytestring crate, so...
    let mut iter = SymbolIter::new(cursor, Vec::new(), 1, stroff, strsize, is_bigend, is_64bit);
    iter.next()
}

/// Parsed relocation entry
#[derive(Debug)]
enum Reloc {
    External {
        addr: u32,
        sym_idx: u32,
        is_pc_relative: bool,
        size: u32,
        type_: u32,
    },
    #[allow(dead_code)]
    Local {
        addr: u32,
        section_idx: u32,
        is_pc_relative: bool,
        size: u32,
        type_: u32,
    },
    #[allow(dead_code)]
    Scattered {
        offset: u32,
        value: u32,
        is_pc_relative: bool,
        size: u32,
        type_: u32,
    },
}
impl Reloc {
    fn parse(is_bigend: bool, entry: [u8; 8]) -> Self {
        assert!(!is_bigend);

        let word1 = u32::from_le_bytes(entry[..4].try_into().unwrap());
        let word2 = u32::from_le_bytes(entry[4..8].try_into().unwrap());

        if (word1 & 0x80000000) != 0 {
            let bitfield = word1;
            let value = word2;

            let offset = bitfield & 0xffffff;
            let type_ = (bitfield >> 24) & 0xf;
            let size = 1 << ((bitfield >> 28) & 3); // log2-encoded pointer size
            let is_pc_relative = ((bitfield >> 30) & 1) == 1;

            Reloc::Scattered {
                offset,
                value,
                is_pc_relative,
                size,
                type_,
            }
        } else {
            let addr = word1;
            let bitfield = word2;

            let sym_or_section_idx = bitfield & 0xffffff;
            let is_pc_relative = ((bitfield >> 24) & 1) == 1;
            let size = 1 << ((bitfield >> 25) & 3); // log2-encoded pointer size
            let is_external = (bitfield >> 27) & 1;
            let type_ = (bitfield >> 28) & 0xf;

            if is_external == 1 {
                Reloc::External {
                    addr,
                    sym_idx: sym_or_section_idx,
                    is_pc_relative,
                    size,
                    type_,
                }
            } else {
                Reloc::Local {
                    addr,
                    section_idx: sym_or_section_idx,
                    is_pc_relative,
                    size,
                    type_,
                }
            }
        }
    }
}

fn cpu_subtype_to_str(ty: cpu_subtype_t) -> &'static str {
    match ty {
        mach_object::CPU_SUBTYPE_ARM_ALL => "armv???",
        mach_object::CPU_SUBTYPE_ARM_V4T => "armv4t",
        mach_object::CPU_SUBTYPE_ARM_V5TEJ => "armv5tej",
        mach_object::CPU_SUBTYPE_ARM_V6 => "armv6",
        mach_object::CPU_SUBTYPE_ARM_XSCALE => "armxscale",
        mach_object::CPU_SUBTYPE_ARM_V7 => "armv7",
        mach_object::CPU_SUBTYPE_ARM_V7F => "armv7f",
        mach_object::CPU_SUBTYPE_ARM_V7S => "armv7s",
        mach_object::CPU_SUBTYPE_ARM_V7K => "armv7k",
        mach_object::CPU_SUBTYPE_ARM_V8 => "armv8",
        _ => panic!("Unexpected cpu subtype: {ty:?}"),
    }
}

// Same ARM32 preference as HyperHLE-Reborn ff8eb827. Capability bits
// are requirements, not flags that may be discarded to force a match.
fn arm_subtype_priority(subtype: cpu_subtype_t) -> Option<u8> {
    match subtype {
        mach_object::CPU_SUBTYPE_ARM_V7 => Some(4),
        mach_object::CPU_SUBTYPE_ARM_V7S => Some(3),
        mach_object::CPU_SUBTYPE_ARM_V7F => Some(2),
        mach_object::CPU_SUBTYPE_ARM_V6 => Some(1),
        mach_object::CPU_SUBTYPE_ARM_ALL => Some(0),
        _ => None,
    }
}

fn validate_segment_ranges(commands: &[MachCommand], file_len: usize) -> Result<(), &'static str> {
    for MachCommand(command, _) in commands {
        if let LoadCommand::Segment {
            fileoff,
            filesize,
            vmsize,
            ..
        } = command
        {
            validate_segment_file_range(
                *fileoff as u64,
                *filesize as u64,
                *vmsize as u64,
                file_len,
            )?;
        }
    }
    Ok(())
}

/// Inspect the FAT directory without recursively parsing unrelated slices.
/// mach_object eagerly parses all slices, so one malformed or unsupported
/// image would otherwise prevent a valid ARM32 fallback from being selected.
fn select_fat_slice(bytes: &[u8]) -> Result<Option<&[u8]>, &'static str> {
    let (big_endian, wide) = match bytes.get(..4) {
        Some([0xca, 0xfe, 0xba, 0xbe]) => (true, false),
        Some([0xbe, 0xba, 0xfe, 0xca]) => (false, false),
        Some([0xca, 0xfe, 0xba, 0xbf]) => (true, true),
        Some([0xbf, 0xba, 0xfe, 0xca]) => (false, true),
        _ => return Ok(None),
    };
    let word = |data: &[u8]| {
        let raw = data.try_into().unwrap();
        if big_endian {
            u32::from_be_bytes(raw)
        } else {
            u32::from_le_bytes(raw)
        }
    };
    let count = word(bytes.get(4..8).ok_or("Truncated FAT header")?) as usize;
    let entry_size = if wide { 32 } else { 20 };
    if count > (bytes.len() - 8) / entry_size {
        return Err("Truncated FAT architecture directory");
    }
    let directory_end = 8 + count * entry_size;
    let mut best = None;
    let mut priority = None;
    for entry in bytes[8..directory_end].chunks_exact(entry_size) {
        let cpu = word(&entry[..4]);
        let subtype = word(&entry[4..8]) as cpu_subtype_t;
        if cpu != mach_object::CPU_TYPE_ARM as u32 {
            continue;
        }
        let Some(rank) = arm_subtype_priority(subtype) else {
            continue;
        };
        let (offset, size) = if wide {
            let long = |data: &[u8]| {
                let raw = data.try_into().unwrap();
                if big_endian {
                    u64::from_be_bytes(raw)
                } else {
                    u64::from_le_bytes(raw)
                }
            };
            (long(&entry[8..16]), long(&entry[16..24]))
        } else {
            (
                u64::from(word(&entry[8..12])),
                u64::from(word(&entry[12..16])),
            )
        };
        let range = offset
            .checked_add(size)
            .and_then(|end| Some(usize::try_from(offset).ok()?..usize::try_from(end).ok()?));
        let Some(range) = range.filter(|r| r.start >= directory_end) else {
            continue;
        };
        let Some(slice) = bytes.get(range) else {
            continue;
        };
        // Only a little-endian 32-bit thin Mach-O can be a candidate.
        // In particular do not let the parser recurse into a nested FAT
        // container before we get a chance to reject it.
        if slice.get(..4) != Some(0xfeedfaceu32.to_le_bytes().as_slice()) {
            continue;
        }
        let Ok(OFile::MachFile { header, commands }) = OFile::parse(&mut Cursor::new(slice)) else {
            continue;
        };
        if header.cputype != mach_object::CPU_TYPE_ARM
            || header.cpusubtype != subtype
            || header.is_bigend()
            || header.is_64bit()
            || validate_segment_ranges(&commands, slice.len()).is_err()
        {
            continue;
        }
        if priority.is_none_or(|previous| rank > previous) {
            best = Some(slice);
            priority = Some(rank);
        }
    }
    best.map(Some).ok_or("FAT binary has no usable ARM32 slice")
}

// Adapted from HyperHLE-Reborn ff8eb827, src/mach_o.rs.
fn validate_segment_file_range(
    offset: u64,
    size: u64,
    virtual_size: u64,
    file_len: usize,
) -> Result<(), &'static str> {
    if size > virtual_size {
        return Err("Mach-O segment file size exceeds virtual size");
    }
    if size == 0 {
        return Ok(());
    }
    let end = offset
        .checked_add(size)
        .ok_or("Mach-O segment range overflows")?;
    if end > file_len as u64 {
        return Err("Mach-O segment extends past end of file");
    }
    Ok(())
}

#[cfg(test)]
mod segment_tests {
    use super::*;

    fn thin(subtype: u32, file_size: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        for word in [0xfeedfaceu32, 12, subtype, 2, 1, 56, 0, 1, 56] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let mut name = [0; 16];
        name[..6].copy_from_slice(b"__TEXT");
        bytes.extend_from_slice(&name);
        for word in [0x4000u32, 0x2000, 0, file_size, 7, 5, 0, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes
    }

    fn fat(slices: &[(u32, Vec<u8>)], wide: bool, little: bool) -> Vec<u8> {
        let word = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let long = |v: u64| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&word(if wide { 0xcafebabf } else { 0xcafebabe }));
        bytes.extend_from_slice(&word(slices.len() as u32));
        let mut offset = 8 + slices.len() * if wide { 32 } else { 20 };
        for (subtype, slice) in slices {
            bytes.extend_from_slice(&word(12));
            bytes.extend_from_slice(&word(*subtype));
            if wide {
                bytes.extend_from_slice(&long(offset as u64));
                bytes.extend_from_slice(&long(slice.len() as u64));
            } else {
                bytes.extend_from_slice(&word(offset as u32));
                bytes.extend_from_slice(&word(slice.len() as u32));
            }
            bytes.extend_from_slice(&word(0));
            if wide {
                bytes.extend_from_slice(&word(0));
            }
            offset += slice.len();
        }
        for (_, slice) in slices {
            bytes.extend_from_slice(slice);
        }
        bytes
    }

    #[test]
    fn fat_falls_back_past_bad_contents_and_truncated_ranges() {
        for wide in [false, true] {
            for little in [false, true] {
                let good = thin(6, 84);
                for bad in [thin(9, 128), vec![0; 84], thin(11, 84)] {
                    let bytes = fat(&[(9, bad), (6, good.clone())], wide, little);
                    assert_eq!(select_fat_slice(&bytes).unwrap(), Some(good.as_slice()));
                }
                let mut bytes = fat(&[(6, good.clone()), (9, thin(9, 84))], wide, little);
                bytes.truncate(bytes.len() - 1);
                let mut memory = Mem::new();
                let loaded = MachO::load_from_bytes(&bytes, &mut memory, "fallback".into(), 0);
                assert!(loaded.is_ok());
                assert_eq!(
                    memory.bytes_at(Ptr::<u8, false>::from_bits(0x4000), 84),
                    &good
                );
            }
        }
    }

    #[test]
    fn fat_ranks_supported_slices_instead_of_file_order() {
        for subtypes in [[0, 6, 10, 11, 9], [9, 11, 10, 6, 0]] {
            let slices: Vec<_> = subtypes.into_iter().map(|s| (s, thin(s, 84))).collect();
            let bytes = fat(&slices, false, false);
            assert_eq!(
                select_fat_slice(&bytes).unwrap(),
                Some(thin(9, 84).as_slice())
            );
        }
    }

    #[test]
    fn unsupported_subtypes_and_capabilities_are_rejected() {
        for subtype in [5, 7, 8, 12, 13, 0x80000009, u32::MAX] {
            let mut memory = Mem::new();
            assert!(MachO::load_from_bytes(
                &thin(subtype, 84),
                &mut memory,
                "unsupported".into(),
                0
            )
            .is_err());
        }
        for subtype in [0, 6, 9, 10, 11] {
            assert!(arm_subtype_priority(subtype as cpu_subtype_t).is_some());
        }
    }

    #[test]
    fn invalid_fat_directories_never_select_nested_or_overflowing_slices() {
        for bytes in [
            vec![0xca, 0xfe, 0xba, 0xbe],
            vec![0xca, 0xfe, 0xba, 0xbe, 0xff, 0xff, 0xff, 0xff],
            fat(
                &[(9, vec![0xca, 0xfe, 0xba, 0xbe, 0, 0, 0, 0])],
                false,
                false,
            ),
        ] {
            assert!(select_fat_slice(&bytes).is_err());
        }
        let mut bytes = fat(&[(9, thin(9, 84))], true, false);
        bytes[16..24].copy_from_slice(&u64::MAX.to_be_bytes());
        assert!(select_fat_slice(&bytes).is_err());
        let mut bytes = fat(&[(9, thin(9, 84))], false, false);
        bytes[16..20].copy_from_slice(&0u32.to_be_bytes());
        assert!(select_fat_slice(&bytes).is_err());
    }

    #[test]
    fn symbol_table_preserves_local_and_external_thumb_names() {
        let mut bytes = thin(9, 141);
        bytes[16..20].copy_from_slice(&2u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&80u32.to_le_bytes());
        for word in [2u32, 24, 108, 2, 132, 9] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        for (strx, external, address) in [(1u32, false, 0x4040u32), (5u32, true, 0x4050u32)] {
            bytes.extend_from_slice(&strx.to_le_bytes());
            bytes.extend_from_slice(&[0x0e | u8::from(external), 0, 8, 0]);
            bytes.extend_from_slice(&address.to_le_bytes());
        }
        bytes.extend_from_slice(b"\0foo\0bar\0");
        let mut memory = Mem::new();
        let image = MachO::load_from_bytes(&bytes, &mut memory, "symbols".into(), 0).unwrap();
        assert_eq!(image.external_symbols.get("bar"), Some(&0x4051));
        assert_eq!(image.external_symbols.get("foo"), None);
        assert_eq!(image.exported_symbols.get("foo"), Some(&0x4041));
        assert_eq!(image.symbol_for_linking(&image, "foo"), Some(0x4041));
    }
    #[test]
    fn segment_bounds_and_virtual_tail() {
        assert!(validate_segment_file_range(0x100, 0x100, 0x1000, 0x200).is_ok());
        assert!(validate_segment_file_range(0x100, 0x100, 0x1000, 0x1ff).is_err());
        assert!(validate_segment_file_range(u64::MAX, 1, 1, usize::MAX).is_err());
        assert!(validate_segment_file_range(0, 2, 1, 2).is_err());
        assert!(validate_segment_file_range(u64::MAX, 0, 0x1000, 0).is_ok());
    }
    #[test]
    fn loader_rejects_truncated_segment_before_mapping() {
        let mut bytes = Vec::new();
        for word in [0xfeedfaceu32, 12, 6, 2, 1, 56, 0, 1, 56] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let mut name = [0; 16];
        name[..6].copy_from_slice(b"__TEXT");
        bytes.extend_from_slice(&name);
        for word in [0x1000u32, 0x2000, 0, 128, 7, 5, 0, 0] {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let mut memory = Mem::new();
        let result = MachO::load_from_bytes(&bytes, &mut memory, "truncated-test".into(), 0);
        assert!(matches!(result, Err(e) if e.contains("extends past end")));
    }
}

impl MachO {
    /// Load the all the sections from a Mach-O binary (provided as `bytes`)
    /// into the guest memory (`into_mem`), and return a struct containing
    /// metadata (e.g. symbols).
    pub fn load_from_bytes(
        bytes: &[u8],
        into_mem: &mut Mem,
        name: String,
        slide_to_address: u32,
    ) -> Result<MachO, &'static str> {
        log_dbg!("Reading {:?}", name);

        if let Some(slice) = select_fat_slice(bytes)? {
            return Self::load_from_bytes(slice, into_mem, name, slide_to_address);
        }

        let mut cursor = Cursor::new(bytes);

        let file = OFile::parse(&mut cursor).map_err(|_| "Could not parse Mach-O file")?;

        let (header, commands) = match file {
            OFile::MachFile { header, commands } => (header, commands),
            OFile::FatFile { .. } | OFile::ArFile { .. } | OFile::SymDef { .. } => {
                return Err("Unexpected Mach-O file kind: not an executable");
            }
        };

        if header.cputype != mach_object::CPU_TYPE_ARM {
            return Err("Executable is not for an ARM CPU!");
        }
        if arm_subtype_priority(header.cpusubtype).is_none() {
            return Err("Unsupported ARM CPU subtype or required capabilities");
        }
        log!(
            "Loading {} slice for {:?}",
            cpu_subtype_to_str(header.cpusubtype),
            name
        );

        let is_bigend = header.is_bigend();
        if is_bigend {
            return Err("Executable is not little-endian!");
        }
        let is_64bit = header.is_64bit();
        if is_64bit {
            return Err("Executable is not 32-bit!");
        }
        // Validate all segments before writing any of them to guest memory.
        validate_segment_ranges(&commands, bytes.len())?;
        let split_segs = (header.flags & mach_object::MH_SPLIT_SEGS) != 0;

        // Info used while parsing file
        let mut first_segment_base: Option<u32> = None;
        let mut first_read_write_segment_base: Option<u32> = None;
        let mut text_segment_base: Option<u32> = None;
        let mut all_sections = Vec::new();
        let mut sym_tab_info: Option<(u32, u32, u32, u32)> = None;
        let mut segment_offsets = Vec::new();
        let mut last_segment_end: u32 = 0;

        // Info used for the result
        let mut dynamic_libraries = Vec::new();
        let mut exported_symbols = HashMap::new();
        let mut external_symbols = HashMap::new();
        let mut indirect_undef_symbols: Vec<Option<String>> = Vec::new();
        let mut external_relocations: Vec<(u32, String)> = Vec::new();
        let mut entry_point_pc: Option<u32> = None;

        let slide = slide_to_address;

        for MachCommand(command, _size) in commands {
            match command {
                LoadCommand::Segment {
                    segname,
                    vmaddr,
                    vmsize,
                    fileoff,
                    filesize,
                    initprot,
                    sections,
                    ..
                } => {
                    let vmaddr: u32 = vmaddr.try_into().unwrap();
                    let vmsize: u32 = vmsize.try_into().unwrap();
                    let filesize: u32 = filesize.try_into().unwrap();

                    if first_segment_base.is_none() {
                        first_segment_base = Some(vmaddr + slide);
                    }
                    if first_read_write_segment_base.is_none()
                        && (initprot & VM_PROT_READ) != 0
                        && (initprot & VM_PROT_WRITE) != 0
                    {
                        first_read_write_segment_base = Some(vmaddr + slide);
                    }

                    let load_me = match &*segname {
                        // Special linker data section, not meant to be loaded.
                        "__LINKEDIT" => false,
                        // Zero page needs to be handled seperately.
                        "__PAGEZERO" => {
                            assert!(vmaddr == 0);
                            assert!(filesize == 0);
                            into_mem.set_null_segment_size(vmsize);
                            false
                        }
                        "__TEXT" => {
                            assert!(text_segment_base.is_none());
                            text_segment_base = Some(vmaddr + slide);
                            true
                        }
                        "__DATA" => true,
                        _ => {
                            log!("Warning: Unexpected segment name: {}", segname);
                            true
                        }
                    };

                    if load_me {
                        log_dbg!(
                            "reserve {} addr {:#x} size {}",
                            segname,
                            vmaddr + slide,
                            vmsize
                        );
                        into_mem.reserve(vmaddr + slide, vmsize);

                        // If filesize is less than vmsize, the rest of the
                        // segment should be filled with zeroes. We are assuming
                        // the memory is already zeroed!
                        if filesize > 0 {
                            assert!(filesize <= vmsize);

                            let src = &bytes[fileoff..][..filesize as usize];
                            let dst =
                                into_mem.bytes_at_mut(Ptr::from_bits(vmaddr + slide), filesize);
                            dst.copy_from_slice(src);
                        }
                    }

                    all_sections.extend_from_slice(&sections);
                    segment_offsets.push(vmaddr);
                    last_segment_end = last_segment_end.max(vmaddr + vmsize + slide);
                }
                LoadCommand::SymTab {
                    symoff,
                    nsyms,
                    stroff,
                    strsize,
                } => {
                    sym_tab_info = Some((symoff, nsyms, stroff, strsize));
                    if cursor.seek(SeekFrom::Start(symoff.into())).is_ok() {
                        let mut cursor = cursor.clone();
                        let symbols = SymbolIter::new(
                            &mut cursor,
                            all_sections.clone(),
                            nsyms,
                            stroff,
                            strsize,
                            is_bigend,
                            is_64bit,
                        );
                        for symbol in symbols {
                            if let Symbol::Debug { .. } = symbol {
                                continue;
                            }
                            if let Symbol::Defined {
                                name: Some(name),
                                external,
                                entry,
                                desc,
                                ..
                            } = symbol
                            {
                                let entry: u32 = entry.try_into().unwrap();
                                let entry = if desc & N_ARM_THUMB_DEF != 0 {
                                    entry | GuestFunction::THUMB_BIT
                                } else {
                                    entry
                                };
                                exported_symbols.insert(name.to_string(), slide + entry);
                                if external {
                                    external_symbols.insert(name.to_string(), slide + entry);
                                }
                            };
                        }
                    }
                }
                LoadCommand::DySymTab {
                    indirectsymoff,
                    nindirectsyms,
                    extreloff,
                    nextrel,
                    ..
                } => {
                    let indirectsyms =
                        &bytes[indirectsymoff as usize..][..nindirectsyms as usize * 4];
                    for idx in indirectsyms.chunks(4) {
                        assert!(!is_bigend);
                        let idx = u32::from_le_bytes(idx.try_into().unwrap());

                        let mut cursor = cursor.clone();
                        let sym = get_sym_by_idx(
                            idx,
                            sym_tab_info.unwrap(),
                            is_bigend,
                            is_64bit,
                            &mut cursor,
                        );
                        indirect_undef_symbols.push(match sym {
                            // apparently used in apps?
                            Some(Symbol::Undefined { name: Some(n), .. }) => Some(String::from(n)),
                            // apparently used in libraries?
                            Some(Symbol::Prebound { name: Some(n), .. }) => Some(String::from(n)),
                            // apparently used within libstdc++ for linking to
                            // itself, e.g. to "__Znwm". might be a PIC thing
                            Some(Symbol::Defined { name: Some(n), .. }) => Some(String::from(n)),
                            None => None,
                            _ => panic!("Unexpected symbol kind {sym:?}"),
                        })
                    }

                    let extrels = &bytes[extreloff as usize..][..nextrel as usize * 8];
                    for entry in extrels.chunks(8) {
                        let reloc = Reloc::parse(is_bigend, entry.try_into().unwrap());
                        let Reloc::External {
                            addr,
                            sym_idx,
                            is_pc_relative: false,
                            size: 4,
                            type_: 0, // generic
                        } = reloc
                        else {
                            panic!("Unhandled extrel: {reloc:?}")
                        };
                        let addr = if split_segs {
                            addr + first_read_write_segment_base.unwrap()
                        } else {
                            addr + first_segment_base.unwrap()
                        };

                        let mut cursor = cursor.clone();
                        let sym = get_sym_by_idx(
                            sym_idx,
                            sym_tab_info.unwrap(),
                            is_bigend,
                            is_64bit,
                            &mut cursor,
                        );
                        assert_eq!(slide, 0); // TODO
                        match sym {
                            Some(Symbol::Undefined { name: Some(n), .. }) => {
                                external_relocations.push((addr, String::from(n)));
                            }
                            Some(Symbol::Defined { entry, desc, .. }) => {
                                // Apparently these are used for internal
                                // (intra-binary) relocations, despite being
                                // in the external section?
                                //
                                // Resolve them immediately, there is no value
                                // in passing these on to Dyld.
                                let addr = Ptr::from_bits(addr);
                                let addend: u32 = into_mem.read(addr);
                                let entry = entry as u32;
                                let entry = if desc & N_ARM_THUMB_DEF != 0 {
                                    entry | GuestFunction::THUMB_BIT
                                } else {
                                    entry
                                };
                                into_mem.write(addr, entry.wrapping_add(addend));
                            }
                            Some(Symbol::Prebound { name: Some(n), .. }) => {
                                let ptr_ptr = Ptr::<u32, true>::from_bits(addr);
                                into_mem.write(ptr_ptr, 0); // Clear prebinding.
                                external_relocations.push((addr, String::from(n)));
                            }
                            _ => panic!("Unexpected symbol kind {sym:?}"),
                        };
                    }
                }
                LoadCommand::EncryptionInfo { id, .. } if id != 0 => {
                    return Err("The executable is encrypted. touchHLE can't run encrypted apps!");
                }
                LoadCommand::LoadDyLib(DyLib { name, .. }) => {
                    dynamic_libraries.push(String::from(&*name));
                }
                // Old-style entry point PC command
                LoadCommand::UnixThread { state, .. } => {
                    let ThreadState::Arm {
                        __r: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                        __sp: 0,
                        __lr: 0,
                        __pc: pc,
                        __cpsr: 0,
                    } = state
                    else {
                        panic!("Unexpected initial thread state in {name:?}: {state:?}");
                    };
                    // There should only be a single initial thread state.
                    assert!(entry_point_pc.is_none());
                    entry_point_pc = Some(pc);
                }
                // New-style entry point PC command
                LoadCommand::EntryPoint {
                    entryoff,
                    stacksize,
                } => {
                    if stacksize != 0 {
                        log!("TODO: stack size of {:#x} bytes requested", stacksize);
                    }
                    // There should only be a single entry point.
                    // (Presumably an executable won't use both commands?)
                    assert!(entry_point_pc.is_none());
                    let entryoff: u32 = entryoff.try_into().unwrap();
                    entry_point_pc = Some(text_segment_base.unwrap() + entryoff);
                }
                // Used in iOS 3.1+ apps. Also contains info about
                // weak and lazy binds, but we already handle those.
                LoadCommand::DyldInfo {
                    rebase_off,
                    rebase_size,
                    bind_off,
                    bind_size,
                    ..
                } => {
                    let rebase_opcodes = Rebase::parse(
                        &bytes[rebase_off as usize..][..rebase_size as usize],
                        size_of::<GuestUSize>(),
                    );

                    for symb in rebase_opcodes {
                        match symb.symbol_type {
                            BindSymbolType::Pointer => {
                                let addr = segment_offsets[symb.segment_index]
                                    + symb.symbol_offset as u32
                                    + slide;
                                let original_location = Ptr::from_bits(addr);
                                let old: u32 = into_mem.read(original_location);
                                log_dbg!(
                                    "Pointer rebase at {:#x} from {:#x} to {:#x}",
                                    addr,
                                    old,
                                    old + slide
                                );
                                into_mem.write(original_location, old + slide);
                            }
                            _ => unimplemented!(
                                "Unhandled DyldInfo rebase symbol type: {:?}",
                                symb.symbol_type
                            ),
                        }
                    }

                    let bind_opcodes = Bind::parse(
                        &bytes[bind_off as usize..][..bind_size as usize],
                        size_of::<GuestUSize>(),
                    );
                    for symb in bind_opcodes {
                        match symb.symbol_type {
                            BindSymbolType::Pointer => {
                                let addr = segment_offsets[symb.segment_index]
                                    + symb.symbol_offset as u32
                                    + slide;
                                log_dbg!("Pointer bind: {:#x} -> {}", addr, symb.name);
                                external_relocations.push((addr, symb.name));
                            }
                            _ => unimplemented!(
                                "Unhandled DyldInfo bind symbol type: {:?}",
                                symb.symbol_type
                            ),
                        }
                    }
                }
                _ => (),
            }
        }

        let sections = all_sections
            .iter()
            .map(|section| {
                let section = &**section;

                let name = section.sectname.clone();
                let addr: u32 = TryInto::<u32>::try_into(section.addr).unwrap() + slide;
                let size: u32 = section.size.try_into().unwrap();
                let type_ = section.flags.sect_type();

                log_dbg!(
                    "Section: {:?} {:#x} ({:#x} bytes), type {}",
                    name,
                    addr,
                    size,
                    type_,
                );

                use SectionType as ST;
                let (type_, dyld_entry_size) = match type_ {
                    S_MOD_INIT_FUNC_POINTERS => (ST::ModInitFuncPointers, None),
                    // Symbol stub sections have a variable size depending on
                    // whether PIC is in use.
                    S_SYMBOL_STUBS => (ST::SymbolStubs, Some(section.reserved2)),
                    // 4 bytes per pointer
                    S_LAZY_SYMBOL_POINTERS => (ST::LazySymbolPointers, Some(4)),
                    S_NON_LAZY_SYMBOL_POINTERS => (ST::NonLazySymbolPointers, Some(4)),
                    _ => (ST::Normal, None),
                };
                let dyld_indirect_symbol_info = dyld_entry_size.map(|entry_size| {
                    let indirect_start = section.reserved1 as usize;
                    assert!(size.is_multiple_of(entry_size));
                    let indirect_count = (size / entry_size) as usize;
                    let indirects = &mut indirect_undef_symbols[indirect_start..][..indirect_count];
                    let syms = indirects.iter_mut().map(|sym| sym.take()).collect();
                    DyldIndirectSymbolInfo {
                        entry_size,
                        indirect_undef_symbols: syms,
                    }
                });

                Section {
                    name,
                    addr,
                    size,
                    type_,
                    dyld_indirect_symbol_info,
                }
            })
            .collect();

        Ok(MachO {
            name,
            dynamic_libraries,
            sections,
            exported_symbols,
            external_symbols,
            external_relocations,
            entry_point_pc,
            last_segment_end,
        })
    }

    /// Preserve self-local linkage without leaking private names from a
    /// different image. Global lookup order remains the caller's choice.
    pub fn symbol_for_linking(&self, requester: &MachO, name: &str) -> Option<u32> {
        if std::ptr::eq(self, requester) {
            self.exported_symbols.get(name).copied()
        } else {
            self.external_symbols.get(name).copied()
        }
    }

    /// Load the all the sections from a Mach-O binary (from `path`) into the
    /// guest memory (`into_mem`), and return a struct containing metadata
    /// (e.g. symbols).
    pub fn load_from_file<P: AsRef<GuestPath>>(
        path: P,
        fs: &Fs,
        into_mem: &mut Mem,
        slide_to_address: u32,
    ) -> Result<MachO, &'static str> {
        let name = path.as_ref().file_name().unwrap().to_string();
        Self::load_from_bytes(
            &fs.read(path.as_ref())
                .map_err(|_| "Could not read executable file")?,
            into_mem,
            name,
            slide_to_address,
        )
    }

    /// Get a section by its name (`&str`) or type ([SectionType]).
    pub fn get_section<P: SectionPredicate>(&self, by: P) -> Option<&Section> {
        self.sections.iter().find(|section| by.test(section))
    }
}
