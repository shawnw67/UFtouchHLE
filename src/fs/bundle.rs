/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! App bundles, and unpacking IPA files into them.
//!
//! Like on iPhone OS, apps always run from an unpacked bundle. The IPA is only
//! used for installing them.
use crate::fs::FsNode;
use crate::libc::time::{calendar_date_to_timestamp, tm};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use zip::ZipArchive;

/// Error from [unpack_ipa] for an IPA file that can't be read at any position,
/// which it needs. Copying the file somewhere else fixes that. The Android
/// launcher checks for this.
const NOT_SEEKABLE_ERROR: &str = "not seekable";

/// Warn about IPA entries that can't be read. The damage is in the file itself
/// (e.g. a truncated or corrupted download), so the app will see these files as
/// missing; naming them makes that diagnosable from a log.
fn log_damaged_entries(count: usize, error: &str) {
    log!(
        "Warning: {} IPA archive entries are damaged ({}) and will be missing from the app bundle",
        count,
        error
    );
}

fn find_bundle_path(zip: &mut ZipArchive<File>) -> Result<String, String> {
    for i in 0..zip.len() {
        let Ok(file) = zip.by_index(i) else {
            continue;
        };
        if let Some(name) = file
            .name()
            .strip_prefix("Payload/")
            .and_then(|path| path.split_once('/'))
            .and_then(|(name, _)| name.strip_suffix(".app"))
        {
            return Ok(format!("Payload/{name}.app"));
        }
    }
    Err("no app bundle found in the IPA archive".to_string())
}

fn modification_time(file: &zip::read::ZipFile) -> SystemTime {
    let modified = file.last_modified();
    // Note: this is consistent with the 'Date' and 'Time' of files inside an
    // IPA archive as reported by 7-zip, and assumes GMT.
    let timestamp: i64 = calendar_date_to_timestamp(tm::from(
        modified.year(),
        modified.month(),
        modified.day(),
        modified.hour(),
        modified.minute(),
        modified.second(),
    ))
    .into();
    SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp.max(0) as u64)
}

const FILE_TYPE_MASK: u32 = 0o170000;
const FILE_TYPE_SYMLINK: u32 = 0o120000;

/// Write one file of an IPA. A symbolic link is added to `links` instead.
fn unpack_file(
    file: &mut zip::read::ZipFile,
    dest: &Path,
    links: &mut Vec<(PathBuf, String)>,
) -> Result<(), String> {
    let error = |e: std::io::Error| format!("Could not unpack {}: {e}", dest.display());
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(error)?;
    }
    if file
        .unix_mode()
        .is_some_and(|mode| mode & FILE_TYPE_MASK == FILE_TYPE_SYMLINK)
    {
        let mut target = String::new();
        file.read_to_string(&mut target).map_err(error)?;
        links.push((dest.to_path_buf(), target));
        return Ok(());
    }

    let modified = modification_time(file);
    let mut out = File::create(dest).map_err(error)?;
    if file.name().ends_with("/sfx.RAW") {
        let (name, crc) = (file.name().to_string(), file.crc32());
        let mut data = Vec::new();
        file.read_to_end(&mut data).map_err(error)?;
        if repair_known_gta_sfx(&name, crc, &mut data) {
            log!("Corrected a known one-byte shift in GTA Vice City's sfx.RAW; the IPA is unchanged.");
        }
        out.write_all(&data).map_err(error)?;
    } else {
        std::io::copy(file, &mut out).map_err(error)?;
    }
    // Apps can look at this, and it isn't important if it fails.
    let _ = out.set_modified(modified);
    Ok(())
}

/// Where an entry of an IPA goes in the unpacked bundle, if it's included.
fn bundle_relative_path<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    // The artwork is outside the bundle, but it's used for the poster.
    if name == "iTunesArtwork" || name == "iTunesArtwork@2x" {
        return Some(name);
    }
    name.strip_prefix(prefix).filter(|path| !path.is_empty())
}

/// Unpack the app bundle in an IPA to `dest`, replacing what's there.
/// `progress` is told how many bytes of how many are done.
pub fn unpack_ipa(
    mut ipa: File,
    dest: &Path,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(), String> {
    if ipa.seek(SeekFrom::End(0)).is_err() {
        return Err(NOT_SEEKABLE_ERROR.to_string());
    }
    let mut zip = ZipArchive::new(ipa).map_err(|e| format!("Could not open IPA archive: {e}"))?;
    let bundle_path = find_bundle_path(&mut zip)?;
    let prefix = format!("{bundle_path}/");

    let mut total = 0;
    for i in 0..zip.len() {
        if let Ok(file) = zip.by_index(i) {
            if bundle_relative_path(file.name(), &prefix).is_some() {
                total += file.size();
            }
        }
    }

    // This is never listed as an app, so a partial unpack won't get run.
    let mut temp_name = dest.as_os_str().to_owned();
    temp_name.push(".importing");
    let temp = PathBuf::from(temp_name);
    if temp.exists() {
        std::fs::remove_dir_all(&temp).map_err(|e| format!("Could not clear old files: {e}"))?;
    }
    std::fs::create_dir_all(&temp).map_err(|e| format!("Could not create directory: {e}"))?;

    let result = (|| {
        let mut done = 0;
        let mut first_error = None;
        let mut damaged_count = 0;
        let mut links = Vec::new();
        for i in 0..zip.len() {
            // Unreadable entries (e.g. with corrupt local file headers) are
            // skipped, and the app sees those files as missing.
            let mut file = match zip.by_index(i) {
                Ok(file) => file,
                Err(e) => {
                    first_error.get_or_insert_with(|| e.to_string());
                    damaged_count += 1;
                    continue;
                }
            };
            let name = file.name().to_string();
            let Some(relative) = bundle_relative_path(&name, &prefix) else {
                continue;
            };
            if relative.split('/').any(|part| part == "..") {
                return Err(format!("Unsafe path in IPA archive: {name}"));
            }
            let path = temp.join(relative);
            if file.is_dir() {
                std::fs::create_dir_all(&path)
                    .map_err(|e| format!("Could not create directory: {e}"))?;
                continue;
            }
            unpack_file(&mut file, &path, &mut links)?;
            done += file.size();
            progress(done, total);
        }
        // Android's shared storage has no symbolic links, so these are always
        // copies. Old apps have one, from CodeResources into _CodeSignature.
        for (link, target) in links {
            let source = link.parent().unwrap_or(&temp).join(&target);
            if std::fs::copy(&source, &link).is_err() {
                log!(
                    "Warning: could not unpack link {} to {}",
                    link.display(),
                    target
                );
            }
        }
        if damaged_count > 0 {
            log_damaged_entries(
                damaged_count,
                first_error.as_deref().unwrap_or("unknown error"),
            );
        }
        if dest.exists() {
            std::fs::remove_dir_all(dest)
                .map_err(|e| format!("Could not replace the old app: {e}"))?;
        }
        std::fs::rename(&temp, dest).map_err(|e| format!("Could not finish unpacking: {e}"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&temp);
    }
    result
}

/// An app bundle, which is a directory.
pub struct BundleData(PathBuf);

impl BundleData {
    pub fn bundle_name(&self) -> &str {
        self.0.file_stem().unwrap().to_str().unwrap()
    }

    pub fn open_host_dir(path: &Path) -> Result<BundleData, String> {
        Ok(BundleData(path.to_path_buf()))
    }

    /// Open a bundle. An IPA gets unpacked next to it, if that wasn't done yet.
    pub fn open_any(path: &Path) -> Result<BundleData, String> {
        let is_ipa = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ipa"));
        if path.is_file() && is_ipa {
            let dest = path.with_extension("app");
            if !dest.is_dir() {
                echo!("Unpacking {} to {}", path.display(), dest.display());
                let ipa = File::open(path).map_err(|e| format!("Could not open IPA file: {e}"))?;
                unpack_ipa(ipa, &dest, &mut |_, _| ())?;
            }
            Self::open_host_dir(&dest)
        } else if path.is_dir() {
            Self::open_host_dir(path)
        } else {
            Err(format!(
                "{} is not a directory or an IPA file",
                path.display()
            ))
        }
    }

    pub(super) fn into_fs_node(self) -> FsNode {
        FsNode::from_host_dir(&self.0, false)
    }

    pub fn read_plist(&mut self) -> Result<Vec<u8>, String> {
        std::fs::read(self.0.join("Info.plist"))
            .map_err(|e| format!("Could not read Info.plist from the app bundle directory: {e}"))
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

#[cfg(test)]
mod unpack_tests {
    use super::*;
    use zip::write::{FileOptions, ZipWriter};

    #[test]
    fn unpacks_bundle_files_only_and_replaces_old_bundle() {
        let dir = std::env::temp_dir().join(format!("podroid-unpack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ipa_path = dir.join("test.ipa");
        let mut zip = ZipWriter::new(File::create(&ipa_path).unwrap());
        let options = FileOptions::default();
        for (name, contents) in [
            ("iTunesArtwork", "art"),
            ("iTunesMetadata.plist", "metadata"),
            ("Payload/Test.app/Info.plist", "plist"),
            ("Payload/Test.app/data/level.pkg", "level"),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(contents.as_bytes()).unwrap();
        }
        zip.add_symlink("Payload/Test.app/link", "data/level.pkg", options)
            .unwrap();
        zip.finish().unwrap();

        let dest = dir.join("test.app");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("old"), "old").unwrap();
        let mut last_progress = (0, 0);
        let ipa = File::open(&ipa_path).unwrap();
        unpack_ipa(ipa, &dest, &mut |done, total| last_progress = (done, total)).unwrap();

        assert_eq!(
            std::fs::read_to_string(dest.join("Info.plist")).unwrap(),
            "plist"
        );
        assert_eq!(std::fs::read_to_string(dest.join("link")).unwrap(), "level");
        assert_eq!(
            std::fs::read_to_string(dest.join("iTunesArtwork")).unwrap(),
            "art"
        );
        assert!(!dest.join("iTunesMetadata.plist").exists());
        assert!(!dest.join("old").exists());
        assert!(!dir.join("test.app.importing").exists());
        assert_eq!(last_progress.0, last_progress.1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
