/*
 * mhp2g_save: Monster Hunter Portable 2nd G save converter
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU General Public License as published by the Free
 * Software Foundation, either version 3 of the License, or (at your option)
 * any later version. See COPYING.
 */
//! Reading, writing and converting Monster Hunter Portable 2nd G saves of
//! the PSP (ULJM-05500) and iOS versions. PSP saves of Monster Hunter
//! Freedom Unite (ULUS-10391, ULES-01213), the same game, can be read too.
//! See README.md.
//!
//! Both platforms keep the same plaintext ([game]), protected by the same
//! game layer apart from the salt of its hash. On top of that, iOS uses
//! AES-128-CBC ([ios]) and the PSP uses its savedata encryption ([psp]).

mod aes;
pub mod game;
pub mod ios;
pub mod psp;

pub use game::Platform;
use game::{
    BODY_LEN, HASH_OFF, HEADER_LEN, IOS_GUILD_CARD_NAME, IOS_TAIL, MONSTER_RECORDS, SEED_OFF,
    SHIFTED, SLOTS, SLOT_LEN, SLOT_NAME_LEN, SLOT_OFF,
};
use std::fmt;

#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Random bits for seeds and file headers (they needn't be secure).
pub(crate) fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.finish()
}

/// How a save file was stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// iOS `savedata.dat`.
    IosFile,
    /// PSP `MHP2NDG.BIN` with the system layer.
    PspFile,
    /// PSP save without the system layer (e.g. from Apollo Save Tool).
    PspGameLayer,
    /// Plaintext from this or another tool.
    Plaintext,
}

#[derive(Debug)]
pub struct Save {
    pub platform: Platform,
    pub format: Format,
    /// 0x16A100 bytes, with a valid hash. (That of a Freedom Unite save from
    /// the PSP is made with the salt of [Platform::Ios].)
    pub plain: Vec<u8>,
}

/// The PSP games whose saves can be read: the game key of each, and the
/// platform whose salt its save hash uses. All of them share the game layer.
/// Freedom Unite (the Western release of MHP2G) hashes its saves with the
/// iOS salt.
const PSP_GAMES: [(&[u8; 16], Platform); 2] = [
    (&psp::MHP2G_JP_KEY, Platform::Psp),
    (&psp::MHFU_KEY, Platform::Ios),
];

/// Identify and decrypt a save of either platform. `psp_params` is
/// `SAVEDATA_PARAMS[0]` from the save's `PARAM.SFO`, if there is one.
pub fn load(raw: &[u8], psp_params: Option<u8>) -> Result<Save, Error> {
    let mut errors = Vec::new();
    let found = |platform, format, plain| {
        Ok(Save {
            platform,
            format,
            plain,
        })
    };
    if raw.len() == ios::FILE_LEN {
        match ios::decrypt(raw) {
            Ok(inner) => {
                let plain = game::decrypt(&inner);
                if game::hash_ok(&plain, Platform::Ios) {
                    return found(Platform::Ios, Format::IosFile, plain);
                }
                errors.push("iOS: decrypted, but the SHA-1 doesn't match".to_string());
            }
            Err(e) => errors.push(format!("iOS: {e}")),
        }
    }
    if raw.len() == BODY_LEN + 16 {
        let params = match psp_params {
            Some(p) => vec![p],
            None => vec![psp::MHP2G_PARAMS, 0x21],
        };
        for p in params {
            // A wrong key doesn't make this fail, it just gives garbage that
            // the hash check rejects.
            for (key, salt_of) in PSP_GAMES {
                let inner = psp::decrypt_file(raw, p, key)?;
                let plain = game::decrypt(&inner);
                if game::hash_ok(&plain, salt_of) {
                    return found(Platform::Psp, Format::PspFile, plain);
                }
            }
        }
        errors.push("PSP: decrypted, but the SHA-1 doesn't match".to_string());
    }
    if raw.len() == BODY_LEN {
        let plain = game::decrypt(raw);
        if game::hash_ok(&plain, Platform::Psp) {
            return found(Platform::Psp, Format::PspGameLayer, plain);
        }
        errors.push("PSP game layer: the SHA-1 doesn't match".to_string());
    }
    if raw.len() == SEED_OFF || raw.len() == BODY_LEN {
        for platform in [Platform::Ios, Platform::Psp] {
            if game::hash_ok(raw, platform) {
                return found(platform, Format::Plaintext, complete_plain(raw, platform)?);
            }
        }
        errors.push("plaintext: the SHA-1 matches neither platform".to_string());
    }
    if errors.is_empty() {
        errors.push(format!(
            "its size (0x{:X}) matches no known format",
            raw.len()
        ));
    }
    Err(Error::new(format!(
        "unrecognized save file:\n  {}",
        errors.join("\n  ")
    )))
}

/// Pad a plaintext from this or another tool (mhef leaves out hash and
/// seed) to 0x16A100 bytes and seal it.
pub fn complete_plain(raw: &[u8], platform: Platform) -> Result<Vec<u8>, Error> {
    if ![HASH_OFF, SEED_OFF, BODY_LEN].contains(&raw.len()) {
        return Err(Error::new(format!(
            "a plaintext save is 0x{HASH_OFF:X}, 0x{SEED_OFF:X} or 0x{BODY_LEN:X} bytes, not 0x{:X}",
            raw.len()
        )));
    }
    let mut plain = raw.to_vec();
    plain.resize(BODY_LEN, 0);
    game::seal(&mut plain, platform, game::random_seed());
    Ok(plain)
}

/// Move a plaintext to another platform. The settings block comes from
/// `template` (a plaintext of the target platform) or the default one.
///
/// The end of each character slot is laid out differently on the two
/// platforms: its parts are moved, and the guild card's monster records are
/// repacked. An iOS guild card without a name gets the character's (the iOS
/// game doesn't fill it in itself); other names are kept.
pub fn convert(plain: &[u8], from: Platform, to: Platform, template: Option<&[u8]>) -> Vec<u8> {
    let mut out = plain.to_vec();
    if from != to {
        let header = to.default_header();
        out[..HEADER_LEN].copy_from_slice(template.map_or(&header[..], |t| &t[..HEADER_LEN]));
        for slot in 0..SLOTS {
            let base = SLOT_OFF + slot * SLOT_LEN;
            out[base + IOS_TAIL.start..base + IOS_TAIL.end].fill(0);
            let (records, delta) = MONSTER_RECORDS;
            for (ios, delta) in SHIFTED.into_iter().chain([(records.clone(), delta)]) {
                let psp = ios.start + delta..ios.end + delta;
                let (src, dst) = if to == Platform::Ios {
                    (psp, ios)
                } else {
                    (ios, psp)
                };
                out[base + dst.start..base + dst.end]
                    .copy_from_slice(&plain[base + src.start..base + src.end]);
            }
            let start = base
                + if to == Platform::Ios {
                    records.start
                } else {
                    records.start + delta
                };
            for record in out[start..start + records.len()].as_chunks_mut::<8>().0 {
                let ab: [u8; 4] = record[..4].try_into().unwrap();
                let packed = if to == Platform::Ios {
                    let (a, b) = (
                        u16::from_le_bytes([ab[0], ab[1]]),
                        u16::from_le_bytes([ab[2], ab[3]]),
                    );
                    u32::from(a & 0x7F) | u32::from(b) << 7
                } else {
                    let v = u32::from_le_bytes(ab);
                    (v & 0x7F) | (v >> 7 & 0xFFFF) << 16
                };
                record[..4].copy_from_slice(&packed.to_le_bytes());
            }
            let card_name = base + IOS_GUILD_CARD_NAME;
            if to == Platform::Ios && out[card_name] == 0 && out[card_name + 1] == 0 {
                // The UTF-16 name, up to its terminator.
                let len = (0..SLOT_NAME_LEN)
                    .step_by(2)
                    .find(|&i| out[base + i] == 0 && out[base + i + 1] == 0)
                    .unwrap_or(SLOT_NAME_LEN);
                out.copy_within(base..base + len, card_name);
            }
        }
    }
    game::seal(&mut out, to, game::random_seed());
    out
}

/// The result of [import_psp].
pub struct Import {
    /// The new iOS `savedata.dat`.
    pub file: Vec<u8>,
    /// The names of the characters in it, in slot order (empty slots left out).
    pub characters: Vec<String>,
}

/// Convert a PSP save for the iOS version, as an import into an app does.
/// `file` is `MHP2NDG.BIN` (with or without the system layer), `sfo` the
/// `PARAM.SFO` next to it if there is one, and `current` the iOS
/// `savedata.dat` it replaces, if any, whose settings block is kept.
pub fn import_psp(
    file: &[u8],
    sfo: Option<&[u8]>,
    current: Option<&[u8]>,
) -> Result<Import, Error> {
    let params = match sfo {
        Some(sfo) => Some(psp::Sfo::parse(sfo.to_vec())?.params()?),
        None => None,
    };
    let save = load(file, params)?;
    if save.platform != Platform::Psp {
        return Err(Error::new("this is an iOS save, not a PSP one"));
    }
    // A broken current save just means the default settings are used.
    let template = current
        .and_then(|current| load(current, None).ok())
        .filter(|current| current.platform == Platform::Ios)
        .map(|current| current.plain);
    let plain = convert(
        &save.plain,
        Platform::Psp,
        Platform::Ios,
        template.as_deref(),
    );
    let characters = (0..SLOTS)
        .map(|slot| game::character_name(&plain[SLOT_OFF + slot * SLOT_LEN - game::NAME_OFF..]))
        .filter(|name| !name.is_empty())
        .collect();
    Ok(Import {
        file: ios_file(&plain),
        characters,
    })
}

/// Plaintext -> iOS `savedata.dat`.
pub fn ios_file(plain: &[u8]) -> Vec<u8> {
    ios::encrypt(&game::encrypt(plain))
}

/// Plaintext -> PSP `MHP2NDG.BIN`, updating the save's `PARAM.SFO`.
pub fn psp_file(plain: &[u8], sfo: &mut psp::Sfo) -> Result<Vec<u8>, Error> {
    let mut header = [0; 16];
    header[..8].copy_from_slice(&random_u64().to_le_bytes());
    header[8..].copy_from_slice(&random_u64().to_le_bytes());
    psp_file_with_header(plain, sfo, header)
}

pub fn psp_file_with_header(
    plain: &[u8],
    sfo: &mut psp::Sfo,
    header: [u8; 16],
) -> Result<Vec<u8>, Error> {
    let (file, hash) = psp::encrypt_file(
        &game::encrypt(plain),
        sfo.params()?,
        &psp::MHP2G_JP_KEY,
        header,
    )?;
    sfo.update(psp::SAVE_FILE_NAME, &hash)?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use game::{synthetic_plain, NAME_OFF};

    #[test]
    fn load_every_format() {
        let ios = synthetic_plain(Platform::Ios);
        let save = load(&ios_file(&ios), None).unwrap();
        assert_eq!(
            (save.platform, save.format),
            (Platform::Ios, Format::IosFile)
        );
        assert_eq!(save.plain, ios);

        let psp = synthetic_plain(Platform::Psp);
        let save = load(&game::encrypt(&psp), None).unwrap();
        assert_eq!(
            (save.platform, save.format),
            (Platform::Psp, Format::PspGameLayer)
        );
        assert_eq!(save.plain, psp);

        let mut sfo = psp::Sfo::parse(psp::tests::synthetic_sfo()).unwrap();
        let file = psp_file(&psp, &mut sfo).unwrap();
        assert_eq!(file.len(), ios::FILE_LEN);
        let save = load(&file, Some(sfo.params().unwrap())).unwrap();
        assert_eq!(
            (save.platform, save.format),
            (Platform::Psp, Format::PspFile)
        );
        assert_eq!(save.plain, psp);
        // Without PARAM.SFO, MHP2G's mode is assumed.
        assert_eq!(load(&file, None).unwrap().plain, psp);

        let save = load(&psp[..SEED_OFF], None).unwrap();
        assert_eq!(
            (save.platform, save.format),
            (Platform::Psp, Format::Plaintext)
        );
        assert_eq!(save.plain[..SEED_OFF], psp[..SEED_OFF]);
    }

    #[test]
    fn load_freedom_unite() {
        // Another game key, and the iOS salt in the hash.
        let mut plain = synthetic_plain(Platform::Psp);
        game::seal(&mut plain, Platform::Ios, 1);
        let sfo = psp::Sfo::parse(psp::tests::synthetic_sfo()).unwrap();
        let params = sfo.params().unwrap();
        let (file, _) =
            psp::encrypt_file(&game::encrypt(&plain), params, &psp::MHFU_KEY, [0xAB; 16]).unwrap();
        let save = load(&file, Some(params)).unwrap();
        assert_eq!(
            (save.platform, save.format),
            (Platform::Psp, Format::PspFile)
        );
        assert_eq!(save.plain, plain);
        assert_eq!(load(&file, None).unwrap().plain, plain);

        // It imports like a save of the Japanese game.
        let import = import_psp(&file, Some(&sfo.0), None).unwrap();
        assert_eq!(import.characters, ["テスト"]);
        let save = load(&import.file, None).unwrap();
        assert_eq!(save.format, Format::IosFile);
        assert_eq!(save.plain[HEADER_LEN..SLOT_OFF], plain[HEADER_LEN..SLOT_OFF]);
    }

    #[test]
    fn import_psp_save() {
        let psp = synthetic_plain(Platform::Psp);
        let mut sfo = psp::Sfo::parse(psp::tests::synthetic_sfo()).unwrap();
        let file = psp_file(&psp, &mut sfo).unwrap();

        let mut current = synthetic_plain(Platform::Ios);
        current[..HEADER_LEN].fill(0x5A);
        game::seal(&mut current, Platform::Ios, 1);
        let current = ios_file(&current);
        let import = import_psp(&file, Some(&sfo.0), Some(&current)).unwrap();
        assert_eq!(import.characters, ["テスト"]);
        let save = load(&import.file, None).unwrap();
        assert_eq!(save.format, Format::IosFile);
        assert_eq!(save.plain[..HEADER_LEN], [0x5A; HEADER_LEN]);
        assert_eq!(save.plain[HEADER_LEN..SLOT_OFF], psp[HEADER_LEN..SLOT_OFF]);

        // Without PARAM.SFO or a current save.
        let import = import_psp(&file, None, None).unwrap();
        let save = load(&import.file, None).unwrap();
        assert_eq!(save.plain[..HEADER_LEN], Platform::Ios.default_header());

        assert!(import_psp(&current, None, None).is_err());
        assert!(import_psp(&[0; 16], None, None).is_err());
    }

    #[test]
    fn load_rejects_corruption() {
        let mut file = game::encrypt(&synthetic_plain(Platform::Psp));
        file[0x3000] ^= 1;
        assert!(load(&file, None).is_err());
        assert!(load(&[0; 100], None).is_err());
    }

    #[test]
    fn convert_both_ways() {
        let mut psp = synthetic_plain(Platform::Psp);
        // In the second character: money (PSP +0x69250, iOS +0x66960), a
        // flag table entry (PSP +0x6A14C, iOS +0x67854), the guild card's
        // comment and name, and a monster record as in a real save. And
        // something in the global area after the slots (PSP only, kept).
        let slot = SLOT_OFF + SLOT_LEN;
        psp[slot + 0x69250..slot + 0x69254].copy_from_slice(&1174809u32.to_le_bytes());
        psp[slot + 0x6A14C] = 0x11;
        psp[slot + 0x672EE] = 0x38;
        psp[slot + 0x672B8] = b'G';
        psp[slot..slot + 4].copy_from_slice(&[0xCF, 0x30, 0xF3, 0x30]);
        psp[slot + 0x67408 + 8..slot + 0x67408 + 16]
            .copy_from_slice(&[0x03, 0x00, 0x15, 0x00, 0xae, 0x20, 0x42, 0x1e]);
        let global = SLOT_OFF + SLOTS * SLOT_LEN + 0x100;
        psp[global] = 0x42;
        game::seal(&mut psp, Platform::Psp, 1);

        let ios = convert(&psp, Platform::Psp, Platform::Ios, None);
        assert_eq!(ios[..HEADER_LEN], Platform::Ios.default_header());
        assert!(game::hash_ok(&ios, Platform::Ios));
        assert_eq!(
            ios[slot + 0x66960..slot + 0x66964],
            1174809u32.to_le_bytes()
        );
        assert_eq!(ios[slot + 0x67854], 0x11);
        assert!(ios[slot + 0x6803C..slot + SLOT_LEN].iter().all(|&b| b == 0));
        assert_eq!(ios[slot + 0x64A68 + 2], 0x38);
        // The guild card keeps its name; the first character's, which has
        // none, gets the character's.
        assert_eq!(ios[slot + IOS_GUILD_CARD_NAME], b'G');
        assert_eq!(
            ios[slot + 0x64B88..slot + 0x64B90],
            [0x83, 0x0a, 0x00, 0x00, 0xae, 0x20, 0x42, 0x1e]
        );
        assert_eq!(ios[global], 0x42);
        let name = SLOT_OFF + IOS_GUILD_CARD_NAME;
        assert_eq!(game::character_name(&ios[name - NAME_OFF..]), "テスト");
        assert_eq!(ios[name + 6..name + SLOT_NAME_LEN], [0; SLOT_NAME_LEN - 6]);
        // Before the tails, nothing moves.
        assert_eq!(ios[HEADER_LEN..SLOT_OFF], psp[HEADER_LEN..SLOT_OFF]);
        for k in 0..SLOTS {
            let front = SLOT_OFF + k * SLOT_LEN..SLOT_OFF + k * SLOT_LEN + IOS_TAIL.start;
            assert_eq!(ios[front.clone()], psp[front]);
        }

        let back = convert(&ios, Platform::Ios, Platform::Psp, None);
        assert_eq!(back[..HEADER_LEN], psp[..HEADER_LEN]);
        assert!(game::hash_ok(&back, Platform::Psp));
        assert_eq!(
            back[slot + 0x69250..slot + 0x69254],
            1174809u32.to_le_bytes()
        );
        assert_eq!(back[slot + 0x6A14C], 0x11);
        // The guild card, back where it was.
        assert_eq!(
            back[slot + 0x64A00..slot + SLOT_LEN],
            psp[slot + 0x64A00..slot + SLOT_LEN]
        );
        assert_eq!(back[name], 0); // No iOS guild card name on the PSP.
        assert_eq!(back[global], 0x42);

        let mut template = synthetic_plain(Platform::Ios);
        template[..HEADER_LEN].fill(0x5A);
        let ios = convert(&psp, Platform::Psp, Platform::Ios, Some(&template));
        assert_eq!(ios[..HEADER_LEN], [0x5A; HEADER_LEN]);
    }
}
