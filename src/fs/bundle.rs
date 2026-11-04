/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! IPA file format support, allowing it to be used as part of the guest
//! filesystem.
use crate::fs::{FsNode, GuestPath};
use crate::libc::time::{calendar_date_to_timestamp, time_t, tm};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use zip::result::ZipError;
use zip::ZipArchive;

/// A helper struct to build an FsNode with files and directories coming in
/// arbitrary order. This is required, because ZIP files are allowed to store
/// entries in arbitrary order.
struct FsNodeBuilder {
    root: FsNode,
}

impl FsNodeBuilder {
    fn new() -> Self {
        Self {
            root: FsNode::dir(),
        }
    }

    fn find_or_make_directory(&mut self, path: &GuestPath) -> &mut FsNode {
        let mut current = &mut self.root;
        for part in path.as_str().split('/') {
            if part.is_empty() {
                continue;
            }
            assert_ne!(part, "..", "unexpected .. in path: {path:?}");
            let FsNode::Directory { children, .. } = current else {
                panic!("expected directory, got {current:?}");
            };

            let next = children.entry(part.to_string()).or_insert_with(FsNode::dir);
            current = next;
        }
        current
    }

    fn add_file(&mut self, path: &GuestPath, node: FsNode) {
        let (parent_name, file_name) = path.parent_and_file_name().unwrap();
        assert_ne!(file_name, "..", "unexpected .. in path: {path:?}");
        let dir = self.find_or_make_directory(parent_name);
        let FsNode::Directory { children, .. } = dir else {
            panic!("expected directory, got {dir:?}");
        };

        children.insert(file_name.to_string(), node);
    }

    fn add_directory(&mut self, path: &GuestPath) {
        self.find_or_make_directory(path);
    }

    fn build(self) -> FsNode {
        self.root
    }
}

/// Warn once about IPA entries that can't be read. The damage is in the file
/// itself (e.g. a truncated or corrupted download), so the app will see these
/// files as missing; naming them makes that diagnosable from a log.
fn log_damaged_entries(count: usize, names: &[&str], error: &str) {
    const SHOWN: usize = 10;
    let mut list = names
        .iter()
        .take(SHOWN)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        list.push_str(&format!(", and {} more", names.len() - SHOWN));
    }
    log!(
        "Warning: {} IPA archive entries are damaged ({}) and will be missing from the app bundle: [{}]. The IPA file is probably corrupt; try downloading it again.",
        count,
        error,
        list
    );
}

/// Represents an open app bundle, either a directory or a zip file.
pub enum BundleData {
    HostDirectory(PathBuf),
    Zip {
        zip: ZipArchive<std::fs::File>,
        /// Path to the app bundle inside the zip file.
        /// It should be `"Payload/<app name>.app"` (no trailing slash!).
        bundle_path: String,
    },
}

impl BundleData {
    fn find_bundle_path_in_archive(zip: &mut ZipArchive<std::fs::File>) -> Result<String, String> {
        for i in 0..zip.len() {
            let file = zip
                .by_index(i)
                .map_err(|e| format!("Could not open IPA archive entry: {e}"))?;
            let path = file.name();
            if let Some(name) = path
                .strip_prefix("Payload/")
                .and_then(|path| path.split_once('/'))
                .and_then(|(name, _)| name.strip_suffix(".app"))
            {
                return Ok(format!("Payload/{name}.app"));
            }
        }
        Err("no app bundle found in the IPA archive".to_string())
    }

    pub fn bundle_name(&self) -> &str {
        match self {
            BundleData::HostDirectory(bundle_path) => {
                bundle_path.file_stem().unwrap().to_str().unwrap()
            }
            BundleData::Zip { bundle_path, .. } => bundle_path
                .rsplit_once('/')
                .unwrap()
                .1
                .strip_suffix(".app")
                .unwrap(),
        }
    }

    pub fn open_host_dir(path: &Path) -> Result<BundleData, String> {
        Ok(BundleData::HostDirectory(path.to_path_buf()))
    }

    pub fn open_ipa(path: &Path) -> Result<BundleData, String> {
        let file =
            std::fs::File::open(path).map_err(|e| format!("Could not open IPA file: {e}"))?;
        let mut zip =
            ZipArchive::new(file).map_err(|e| format!("Could not open IPA archive: {e}"))?;
        let bundle_path = Self::find_bundle_path_in_archive(&mut zip)?;
        Ok(BundleData::Zip { zip, bundle_path })
    }

    pub fn open_any(path: &Path) -> Result<BundleData, String> {
        if path.is_file()
            && path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("ipa"))
                .unwrap_or(false)
        {
            Ok(Self::open_ipa(path)?)
        } else if path.is_dir() {
            Ok(Self::open_host_dir(path)?)
        } else {
            Err(format!(
                "{} is not a directory or an IPA file",
                path.display()
            ))
        }
    }

    pub(super) fn into_fs_node(self) -> FsNode {
        match self {
            BundleData::HostDirectory(path) => FsNode::from_host_dir(&path, false),
            BundleData::Zip { zip, bundle_path } => {
                let archive = Rc::new(RefCell::new(zip));
                let archive_cache = Rc::new(RefCell::new(HashMap::new()));
                let metadata_map = Rc::new(RefCell::new(HashMap::new()));

                let mut archive_guard = (*archive).borrow_mut();

                let mut builder = FsNodeBuilder::new();
                // Central-directory names of the entries that could be read,
                // used to name the damaged ones afterwards.
                let mut readable_names = HashSet::new();
                let mut first_error = None;
                let mut damaged_count = 0;
                for i in 0..archive_guard.len() {
                    // Unreadable entries (e.g. with corrupt local file
                    // headers) are skipped rather than causing a panic; the
                    // rest of the bundle is still loaded, and the app sees
                    // the skipped files as missing.
                    let file = match archive_guard.by_index(i) {
                        Ok(file) => file,
                        Err(e) => {
                            first_error.get_or_insert_with(|| e.to_string());
                            damaged_count += 1;
                            continue;
                        }
                    };
                    let name = file.name();
                    readable_names.insert(name.to_string());
                    if let Some(path) = name.strip_prefix(&bundle_path) {
                        let path = GuestPath::new(path);
                        if file.is_dir() {
                            builder.add_directory(path);
                        } else {
                            builder.add_file(
                                path,
                                FsNode::bundle_zip_file(IpaFileRef {
                                    archive: archive.clone(),
                                    archive_files_cache: archive_cache.clone(),
                                    metadata_map: metadata_map.clone(),
                                    index: i,
                                }),
                            );
                        }
                    }
                }
                if damaged_count > 0 {
                    let damaged: Vec<&str> = archive_guard
                        .file_names()
                        .filter(|name| !readable_names.contains(*name))
                        .collect();
                    log_damaged_entries(
                        damaged_count,
                        &damaged,
                        first_error.as_deref().unwrap_or("unknown error"),
                    );
                }
                builder.build()
            }
        }
    }

    pub fn read_plist(&mut self) -> Result<Vec<u8>, String> {
        match self {
            BundleData::HostDirectory(path) => {
                std::fs::read(path.join("Info.plist")).map_err(|e| {
                    format!("Could not read Info.plist from the app bundle directory: {e}")
                })
            }
            BundleData::Zip { zip, bundle_path } => {
                let mut file = zip
                    .by_name(&format!("{bundle_path}/Info.plist"))
                    .map_err(|e| format!("Could not open Info.plist from the IPA archive: {e}"))?;
                let mut buf = Vec::new();
                file.read_to_end(&mut buf)
                    .map_err(|e| format!("Could not read Info.plist from the IPA archive: {e}"))?;
                Ok(buf)
            }
        }
    }
}

#[derive(Debug)]
struct ArchivedFileMetadata {
    /// Unix timestamp of file modification
    last_modified: i64,
    /// Uncompressed file size
    size: u64,
}

/// Shared (refcounted) copy of the decompressed version of a file in an IPA.
///
/// Seeking in compressed files is hard, so the simple solution is to read the
/// whole file into memory. This is shared so having multiple copies of the same
/// file open won't waste memory.
type DecompressedFile = Rc<[u8]>;

/// Represents a file inside an IPA bundle that can be opened.
#[derive(Debug)]
pub struct IpaFileRef {
    archive: Rc<RefCell<ZipArchive<std::fs::File>>>,
    archive_files_cache: Rc<RefCell<HashMap<usize, DecompressedFile>>>,
    metadata_map: Rc<RefCell<HashMap<usize, ArchivedFileMetadata>>>,
    index: usize,
}

impl IpaFileRef {
    pub fn open(&self) -> IpaFile {
        // Some games, like THPS2, use a single resource bundle file which is
        // opened each time a new game resource is being read.
        // As IPA is basically an archive, this pattern requires unzipping to be
        // done each time, which is extremely slow.
        // The solution here is to cache unzipped data in memory, which should
        // be OK as early iOS IPA files are relatively small in size.
        let mut archive_cache = (*self.archive_files_cache).borrow_mut();
        archive_cache.entry(self.index).or_insert_with(|| {
            let mut archive = (*self.archive).borrow_mut();
            let mut file = match archive.by_index(self.index) {
                Ok(file) => file,
                Err(ZipError::Io(e)) => {
                    // this is a runtime error, which we __probably__ should not
                    // bubble up to the guest
                    panic!("IO error while opening file from IPA bundle: {e}")
                }
                // anything other than IO error is a bug in the code, we should
                // always have a valid index
                Err(e) => panic!("BUG: could not open file from IPA bundle: {e}"),
            };
            let mut metadata_map = (*self.metadata_map).borrow_mut();
            assert!(!metadata_map.contains_key(&self.index));
            let modified = file.last_modified();
            // This is not the cleanest way!
            // TODO: just use `time` or `chrono` crates for time conversions
            // (this also entails a lot of refactoring in [crate::libc:time])
            let tm = tm::from(
                modified.year(),
                modified.month(),
                modified.day(),
                modified.hour(),
                modified.minute(),
                modified.second(),
            );
            let timestamp = calendar_date_to_timestamp(tm);
            metadata_map.insert(
                self.index,
                ArchivedFileMetadata {
                    last_modified: timestamp.into(),
                    size: file.size(),
                },
            );
            let mut buf = Vec::new();
            file.read_to_end(&mut buf).unwrap();
            if repair_known_gta_sfx(file.name(), file.crc32(), &mut buf) {
                metadata_map.get_mut(&self.index).unwrap().size = buf.len() as u64;
                log!("Corrected a known one-byte shift in GTA Vice City's sfx.RAW in memory; the IPA is unchanged.");
            }
            Rc::from(buf)
        });
        let cached_file = Rc::clone(archive_cache.get(&self.index).unwrap());
        IpaFile {
            file: Cursor::new(cached_file),
        }
    }

    pub fn get_last_modified(&self) -> time_t {
        if !self.metadata_map.borrow().contains_key(&self.index) {
            // This will force metadata loading
            // TODO: get metadata without reading the file
            _ = self.open();
        }
        self.metadata_map
            .borrow()
            .get(&self.index)
            .unwrap()
            .last_modified
            .try_into()
            .unwrap()
    }
    pub fn get_size(&self) -> u64 {
        if !self.metadata_map.borrow().contains_key(&self.index) {
            // This will force metadata loading
            // TODO: get metadata without reading the file
            _ = self.open();
        }
        self.metadata_map.borrow().get(&self.index).unwrap().size
    }
}

/// One known GTA archive contains an extra byte in effect 7438, shifting all
/// subsequent 16-bit effects. Match the complete ZIP CRC, length, path and
/// surrounding bytes before correcting the decompressed cache. Other archives
/// and correctly aligned sound libraries must not be altered.
fn repair_known_gta_sfx(name: &str, crc: u32, data: &mut Vec<u8>) -> bool {
    if name != "Payload/gta3vc.app/sfx.RAW" || crc != 0xd314375c || data.len() != 340_245_503 {
        return false;
    }
    remove_checked_byte(
        data,
        243_258_231,
        &[
            0xe1, 0x0b, 0x01, 0xb9, 0xfd, 0xeb, 0xff, 0x0d, 0x61, 0x24, 0xfe, 0xe4, 0x0c, 0x08,
            0xec, 0xe2, 0x72,
        ],
    )
}

fn remove_checked_byte(data: &mut Vec<u8>, offset: usize, guard: &[u8]) -> bool {
    let Some(start) = offset.checked_sub(guard.len() / 2) else {
        return false;
    };
    let Some(end) = start.checked_add(guard.len()) else {
        return false;
    };
    if data.get(start..end) != Some(guard) || offset >= data.len() {
        return false;
    }
    data.remove(offset);
    true
}

#[cfg(test)]
mod sound_library_repair_tests {
    use super::*;

    #[test]
    fn repair_preserves_prefix_and_suffix_and_requires_expected_bytes() {
        let original = vec![1, 2, 3, 99, 4, 5, 6];
        let mut data = original.clone();
        assert!(!remove_checked_byte(&mut data, 3, &[3, 98, 4]));
        assert_eq!(data, original);
        assert!(remove_checked_byte(&mut data, 3, &[3, 99, 4]));
        assert_eq!(data, [1, 2, 3, 4, 5, 6]);
        assert!(!remove_checked_byte(&mut data, 3, &[3, 99, 4]));
        assert!(!remove_checked_byte(&mut data, usize::MAX, &[1]));
    }

    #[test]
    fn unrelated_or_differently_sized_libraries_are_unchanged() {
        let mut data = vec![1, 2, 3];
        assert!(!repair_known_gta_sfx(
            "Payload/Other.app/sfx.RAW",
            0xd314375c,
            &mut data
        ));
        assert!(!repair_known_gta_sfx(
            "Payload/gta3vc.app/sfx.RAW",
            0,
            &mut data
        ));
        assert!(!repair_known_gta_sfx(
            "Payload/gta3vc.app/sfx.RAW",
            0xd314375c,
            &mut data
        ));
        assert_eq!(data, [1, 2, 3]);
    }
}

/// Represents an opened file in an IPA bundle.
pub struct IpaFile {
    file: Cursor<DecompressedFile>,
}

impl Debug for IpaFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IpaFile")
            .field("size", &self.file.get_ref().len())
            .finish()
    }
}

impl Read for IpaFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buf)
    }
}

impl std::io::Seek for IpaFile {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.file.seek(pos)
    }
}
