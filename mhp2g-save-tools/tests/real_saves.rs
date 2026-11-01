/*
 * mhp2g_save: Monster Hunter Portable 2nd G save converter
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU General Public License as published by the Free
 * Software Foundation, either version 3 of the License, or (at your option)
 * any later version. See COPYING.
 */
//! Checks against real saves, which aren't part of the repository. Each test
//! is skipped unless its environment variable is set:
//!
//! - `MHP2G_PSP_SAVE_DIR`: a PSP save directory (MHP2NDG.BIN, PARAM.SFO)
//!   written by a PSP or emulator.
//! - `MHP2G_ENDECRYPTER_DIR`: `game.bin` (a PSP save without the system
//!   layer), and `MHP2NDG.BIN` and `PARAM.SFO` from
//!   `endecrypter game.bin key.bin PARAM.SFO`, run on a copy of the
//!   PARAM.SFO from `MHP2G_PSP_SAVE_DIR`.
//! - `MHP2G_IOS_SAVE`: an iOS savedata.dat written by the game.
//! - `MHP2G_MHFU_SAVE_DIR`: a save directory (MHP2NDG.BIN, PARAM.SFO) of
//!   Monster Hunter Freedom Unite for the PSP (ULUS10391 or ULES01213).
//!
//! Run with `cargo test -p mhp2g_save -- --nocapture`.

use mhp2g_save::psp::{Sfo, SAVE_FILE_NAME, SFO_FILE_NAME};
use mhp2g_save::{game, Format, Platform};
use std::path::PathBuf;

fn env_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os(name).map(PathBuf::from);
    if path.is_none() {
        eprintln!("{name} isn't set, skipping");
    }
    path
}

fn read(path: PathBuf) -> Vec<u8> {
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn file_hash(sfo: &Sfo) -> Vec<u8> {
    let start = sfo.0.windows(11).position(|w| w == b"MHP2NDG.BIN").unwrap();
    sfo.0[start + 0x0D..start + 0x1D].to_vec()
}

#[test]
fn psp_save_from_a_psp() {
    let Some(dir) = env_path("MHP2G_PSP_SAVE_DIR") else {
        return;
    };
    let file = read(dir.join(SAVE_FILE_NAME));
    let mut sfo = Sfo::parse(read(dir.join(SFO_FILE_NAME))).unwrap();
    sfo.check_params_hashes().unwrap();
    let save = mhp2g_save::load(&file, Some(sfo.params().unwrap())).unwrap();
    assert_eq!(
        (save.platform, save.format),
        (Platform::Psp, Format::PspFile)
    );
    eprintln!("character: {}", game::character_name(&save.plain));

    // Encrypting the same data with the same header gives the same file and
    // the same file hash as the PSP's.
    let original_hash = file_hash(&sfo);
    let again =
        mhp2g_save::psp_file_with_header(&save.plain, &mut sfo, file[..16].try_into().unwrap())
            .unwrap();
    assert!(again == file, "file differs");
    assert_eq!(file_hash(&sfo), original_hash);
    sfo.check_params_hashes().unwrap();
}

#[test]
fn psp_freedom_unite_save() {
    let Some(dir) = env_path("MHP2G_MHFU_SAVE_DIR") else {
        return;
    };
    let file = read(dir.join(SAVE_FILE_NAME));
    let sfo = Sfo::parse(read(dir.join(SFO_FILE_NAME))).unwrap();
    let save = mhp2g_save::load(&file, Some(sfo.params().unwrap())).unwrap();
    assert_eq!(
        (save.platform, save.format),
        (Platform::Psp, Format::PspFile)
    );
    // The hash of these saves is made with the iOS salt.
    assert!(game::hash_ok(&save.plain, Platform::Ios));

    let import = mhp2g_save::import_psp(&file, Some(&sfo.0), None).unwrap();
    eprintln!("characters: {:?}", import.characters);
    assert!(!import.characters.is_empty());
    let ios = mhp2g_save::load(&import.file, None).unwrap();
    assert_eq!((ios.platform, ios.format), (Platform::Ios, Format::IosFile));
}

#[test]
fn psp_save_from_endecrypter() {
    let Some(dir) = env_path("MHP2G_ENDECRYPTER_DIR") else {
        return;
    };
    let game_layer = read(dir.join("game.bin"));
    let expected_file = read(dir.join(SAVE_FILE_NAME));
    let expected_sfo = read(dir.join(SFO_FILE_NAME));
    let plain = game::decrypt(&game_layer);
    assert!(game::hash_ok(&plain, Platform::Psp));

    // Starting from endecrypter's PARAM.SFO, everything but the
    // SAVEDATA_PARAMS hashes must come out the same. Those hashes chain
    // from the one at 0x20, which depends on the fuse ID the tool uses, so
    // check them on their own.
    Sfo::parse(expected_sfo.clone())
        .unwrap()
        .check_params_hashes()
        .unwrap();
    let mut sfo = Sfo::parse(expected_sfo.clone()).unwrap();
    let header = expected_file[..16].try_into().unwrap();
    let file = mhp2g_save::psp_file_with_header(&plain, &mut sfo, header).unwrap();
    assert!(file == expected_file, "file differs");
    sfo.check_params_hashes().unwrap();
    let params = sfo.find("SAVEDATA_PARAMS").unwrap().start;
    let diff: Vec<usize> = (0..expected_sfo.len())
        .filter(|&i| sfo.0[i] != expected_sfo[i])
        .filter(|&i| !(params + 0x10..params + 0x80).contains(&i))
        .collect();
    assert!(diff.is_empty(), "PARAM.SFO differs at {diff:x?}");
}

#[test]
fn ios_save_from_the_game() {
    let Some(path) = env_path("MHP2G_IOS_SAVE") else {
        return;
    };
    let file = read(path);
    let save = mhp2g_save::load(&file, None).unwrap();
    assert_eq!(
        (save.platform, save.format),
        (Platform::Ios, Format::IosFile)
    );
    // Encrypting with the same seed gives the same file.
    assert!(mhp2g_save::ios_file(&save.plain) == file);
}
