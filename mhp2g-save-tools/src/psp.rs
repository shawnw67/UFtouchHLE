/*
 * mhp2g_save: Monster Hunter Portable 2nd G save converter
 *
 * This file is a Rust port of the PSP savedata encryption (sceChnnlsv) and
 * KIRK code in psp-savedata-endecrypter
 * (https://github.com/cielavenir/psp-savedata-endecrypter), as included in
 * Apollo Save Tool for PSP. That code comes from:
 * - jpcsp (C) jpcsp team, especially CryptoEngine by hykem
 * - kirk-engine (C) draan / proxima, with help from coyotebean, Davee,
 *   hitchhikr, kgsws, liquidzigong, Mathieulh, Proxima and SilverSpring
 * - ported to C by popsdeco (aka @cielavenir), hashing fixed with reference
 *   to SED-PC; Apollo Save Tool by bucanero
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU General Public License as published by the Free
 * Software Foundation, either version 3 of the License, or (at your option)
 * any later version. See COPYING.
 */
//! The PSP system layer of save files (`sceChnnlsv`, built on KIRK), and the
//! hashes in `PARAM.SFO` that go with it.
//!
//! An encrypted save file is a 16-byte header followed by the data XORed
//! with a key stream, which is the CBC decryption of a numbered list of
//! blocks derived from the header and the game's key. The file's hash in
//! `SAVEDATA_FILE_LIST` and the hashes in `SAVEDATA_PARAMS` are AES-CMACs.

use crate::aes::{decrypt_block, encrypt_block};
use crate::Error;

/// The game key of Monster Hunter Portable 2nd G (ULJM-05500).
pub const MHP2G_JP_KEY: [u8; 16] = [
    0xcd, 0x1f, 0x20, 0x59, 0xae, 0x70, 0xef, 0x68, 0xdc, 0xa2, 0x45, 0x13, 0xb4, 0x5a, 0xdb, 0x0a,
];
/// The game key of Monster Hunter Freedom Unite (ULUS-10391 and ULES-01213,
/// which share it). It was solved from the file hash in the `PARAM.SFO` of
/// real saves. Its game layer is the same as MHP2G's, but hashed with the
/// salt that iOS uses.
pub const MHFU_KEY: [u8; 16] = [
    0x4a, 0x1f, 0xf3, 0x59, 0xae, 0xb6, 0xef, 0xf8, 0x1c, 0xa8, 0xcb, 0x23, 0xbc, 0xa5, 0x7b, 0xb3,
];
pub const SAVE_FILE_NAME: &str = "MHP2NDG.BIN";
pub const SFO_FILE_NAME: &str = "PARAM.SFO";
/// `SAVEDATA_PARAMS[0]` of MHP2G saves: encryption mode 5.
pub const MHP2G_PARAMS: u8 = 0x41;

const SD_HASH_KEY2: [u8; 16] = [
    0xFA, 0xAA, 0x50, 0xEC, 0x2F, 0xDE, 0x54, 0x93, 0xAD, 0x14, 0xB2, 0xCE, 0xA5, 0x30, 0x05, 0xDF,
];
const SD_HASH_KEY3: [u8; 16] = [
    0x36, 0xA5, 0x3E, 0xAC, 0xC5, 0x26, 0x9E, 0xA3, 0x83, 0xD9, 0xEC, 0x25, 0x6C, 0x48, 0x48, 0x72,
];
const SD_HASH_KEY4: [u8; 16] = [
    0xD8, 0xC0, 0xB0, 0xF3, 0x3E, 0x6B, 0x76, 0x85, 0xFD, 0xFB, 0x4D, 0x7D, 0x45, 0x1E, 0x92, 0x03,
];
const SD_HASH_KEY5: [u8; 16] = [
    0xCB, 0x15, 0xF4, 0x07, 0xF9, 0x6A, 0x52, 0x3C, 0x04, 0xB9, 0xB2, 0xEE, 0x5C, 0x53, 0xFA, 0x86,
];
const SD_HASH_KEY6: [u8; 16] = [
    0x70, 0x44, 0xA3, 0xAE, 0xEF, 0x5D, 0xA5, 0xF2, 0x85, 0x7F, 0xF2, 0xD6, 0x94, 0xF5, 0x36, 0x3B,
];
const SD_HASH_KEY7: [u8; 16] = [
    0xEC, 0x6D, 0x29, 0x59, 0x26, 0x35, 0xA5, 0x7F, 0x97, 0x2A, 0x0D, 0xBC, 0xA3, 0x26, 0x33, 0x00,
];

/// KIRK master key, used to derive the per-console key of KIRK command 5.
const KIRK_MASTER_KEY: [u8; 16] = [
    0x47, 0x5E, 0x09, 0xF4, 0xA2, 0x37, 0xDA, 0x9B, 0xEF, 0xFF, 0x3B, 0xC0, 0x77, 0x14, 0x3D, 0x8A,
];

/// The entries of the KIRK key vault (commands 4 and 7) that savedata uses.
fn kirk_key(seed: u32) -> &'static [u8; 16] {
    match seed {
        0x03 => &[
            0x98, 0x02, 0xC4, 0xE6, 0xEC, 0x9E, 0x9E, 0x2F, 0xFC, 0x63, 0x4C, 0xE4, 0x2F, 0xBB,
            0x46, 0x68,
        ],
        0x04 => &[
            0x99, 0x24, 0x4C, 0xD2, 0x58, 0xF5, 0x1B, 0xCB, 0xB0, 0x61, 0x9C, 0xA7, 0x38, 0x30,
            0x07, 0x5F,
        ],
        0x05 => &[
            0x02, 0x25, 0xD7, 0xBA, 0x63, 0xEC, 0xB9, 0x4A, 0x9D, 0x23, 0x76, 0x01, 0xB3, 0xF6,
            0xAC, 0x17,
        ],
        0x0C => &[
            0x84, 0x85, 0xC8, 0x48, 0x75, 0x08, 0x43, 0xBC, 0x9B, 0x9A, 0xEC, 0xA7, 0x9C, 0x7F,
            0x60, 0x18,
        ],
        0x0D => &[
            0xB5, 0xB1, 0x6E, 0xDE, 0x23, 0xA9, 0x7B, 0x0E, 0xA1, 0x7C, 0xDB, 0xA2, 0xDC, 0xDE,
            0xC4, 0x6E,
        ],
        0x0E => &[
            0xC8, 0x71, 0xFD, 0xB3, 0xBC, 0xC5, 0xD2, 0xF2, 0xE2, 0xD7, 0x72, 0x9D, 0xDF, 0x82,
            0x68, 0x82,
        ],
        0x10 => &[
            0x32, 0x29, 0x5B, 0xD5, 0xEA, 0xF7, 0xA3, 0x42, 0x16, 0xC8, 0x8E, 0x48, 0xFF, 0x50,
            0xD3, 0x71,
        ],
        0x11 => &[
            0x46, 0xF2, 0x5E, 0x8E, 0x4D, 0x2A, 0xA5, 0x40, 0x73, 0x0B, 0xC4, 0x6E, 0x47, 0xEE,
            0x6F, 0x0A,
        ],
        0x12 => &[
            0x5D, 0xC7, 0x11, 0x39, 0xD0, 0x19, 0x38, 0xBC, 0x02, 0x7F, 0xDD, 0xDC, 0xB0, 0x83,
            0x7D, 0x9D,
        ],
        0x53 => &[
            0xAF, 0xFE, 0x8E, 0xB1, 0x3D, 0xD1, 0x7E, 0xD8, 0x0A, 0x61, 0x24, 0x1C, 0x95, 0x92,
            0x56, 0xB6,
        ],
        0x57 => &[
            0x1C, 0x9B, 0xC4, 0x90, 0xE3, 0x06, 0x64, 0x81, 0xFA, 0x59, 0xFD, 0xB6, 0x00, 0xBB,
            0x28, 0x70,
        ],
        0x64 => &[
            0x03, 0xB3, 0x02, 0xE8, 0x5F, 0xF3, 0x81, 0xB1, 0x3B, 0x8D, 0xAA, 0x2A, 0x90, 0xFF,
            0x5E, 0x61,
        ],
        _ => unreachable!("KIRK key 0x{seed:X}"),
    }
}

fn xor16(a: &mut [u8; 16], b: &[u8; 16]) {
    for (x, y) in a.iter_mut().zip(b) {
        *x ^= y;
    }
}

fn encrypted(key: &[u8; 16], mut block: [u8; 16]) -> [u8; 16] {
    encrypt_block(key, &mut block);
    block
}

fn decrypted(key: &[u8; 16], mut block: [u8; 16]) -> [u8; 16] {
    decrypt_block(key, &mut block);
    block
}

/// Encryption mode used by `sceChnnlsv`, from `SAVEDATA_PARAMS[0]`.
pub fn mode_from_params(params: u8, game_key: Option<&[u8; 16]>) -> Result<u32, Error> {
    if game_key.is_none() {
        Ok(1)
    } else if params & 0x20 != 0 {
        Ok(3)
    } else if params & 0x40 != 0 {
        Ok(5)
    } else {
        Err(Error::new(format!(
            "unsupported savedata encryption (SAVEDATA_PARAMS 0x{params:02X})"
        )))
    }
}

/// KIRK key for the hashes of each mode.
fn mode_seed(mode: u32) -> u32 {
    match mode {
        1 => 0x03,
        2 => 0x05,
        3 => 0x0C,
        4 => 0x0D,
        6 => 0x11,
        _ => 0x10,
    }
}

/// XOR data (a multiple of 16 bytes) with the key stream of a file header.
fn crypt_data(mode: u32, header: &[u8; 16], game_key: Option<&[u8; 16]>, data: &mut [u8]) {
    assert!(data.len().is_multiple_of(16));
    let mut h = *header;
    if let Some(game_key) = game_key {
        xor16(&mut h, game_key);
    }
    let (base, list_seed) = match mode {
        1 => (decrypted(kirk_key(0x04), h), 0x53),
        3 => {
            xor16(&mut h, &SD_HASH_KEY4);
            let mut k = decrypted(kirk_key(0x0E), h);
            xor16(&mut k, &SD_HASH_KEY3);
            (k, 0x57)
        }
        5 => {
            xor16(&mut h, &SD_HASH_KEY7);
            let mut k = decrypted(kirk_key(0x12), h);
            xor16(&mut k, &SD_HASH_KEY6);
            (k, 0x64)
        }
        _ => unreachable!(),
    };
    // The key stream is the CBC decryption of a list of blocks numbered from
    // 1, each made of the first 12 bytes of base and the number.
    let list_key = kirk_key(list_seed);
    let mut prev = [0; 16];
    for (i, chunk) in data.as_chunks_mut::<16>().0.iter_mut().enumerate() {
        let mut block = base;
        block[12..].copy_from_slice(&(i as u32 + 1).to_le_bytes());
        let mut stream = decrypted(list_key, block);
        xor16(&mut stream, &prev);
        prev = block;
        for (d, s) in chunk.iter_mut().zip(stream) {
            *d ^= s;
        }
    }
}

/// AES-CMAC (RFC 4493).
fn cmac(key: &[u8; 16], data: &[u8]) -> [u8; 16] {
    fn double(block: [u8; 16]) -> [u8; 16] {
        let v = u128::from_be_bytes(block);
        let carry = if v >> 127 != 0 { 0x87 } else { 0 };
        ((v << 1) ^ carry).to_be_bytes()
    }
    let k1 = double(encrypted(key, [0; 16]));
    let k2 = double(k1);
    let split = if data.is_empty() {
        0
    } else {
        (data.len() - 1) / 16 * 16
    };
    let mut state = [0; 16];
    for block in data[..split].as_chunks::<16>().0 {
        xor16(&mut state, block);
        state = encrypted(key, state);
    }
    let rest = &data[split..];
    let mut last = [0; 16];
    last[..rest.len()].copy_from_slice(rest);
    if rest.len() == 16 {
        xor16(&mut last, &k1);
    } else {
        last[rest.len()] = 0x80;
        xor16(&mut last, &k2);
    }
    xor16(&mut state, &last);
    encrypted(key, state)
}

/// The key of KIRK command 5, derived from the console's fuse ID.
fn fuse_key(fuse_id: u64) -> [u8; 16] {
    let mut fuse = [0; 8];
    fuse[..4].copy_from_slice(&((fuse_id >> 32) as u32).to_be_bytes());
    fuse[4..].copy_from_slice(&(fuse_id as u32).to_be_bytes());
    let mut sub1 = [0; 16];
    for (i, b) in sub1.iter_mut().enumerate() {
        *b = fuse[i % 8];
    }
    let mut sub2 = sub1;
    for _ in 0..3 {
        sub1 = encrypted(&KIRK_MASTER_KEY, sub1);
        sub2 = decrypted(&KIRK_MASTER_KEY, sub2);
    }
    let mut mesh = [[0; 16]; 3];
    for m in &mut mesh {
        for _ in 0..3 {
            sub2 = encrypted(&sub1, sub2);
        }
        *m = sub2;
    }
    encrypted(&mesh[2], mesh[1])
}

/// The hash `sceChnnlsv` makes of data (`sceSdGetLastIndex`).
fn sd_hash(mode: u32, data: &[u8], game_key: Option<&[u8; 16]>) -> [u8; 16] {
    let key = kirk_key(mode_seed(mode));
    let mut hash = cmac(key, data);
    match mode {
        5 | 6 => xor16(&mut hash, &SD_HASH_KEY5),
        3 | 4 => xor16(&mut hash, &SD_HASH_KEY2),
        _ => (),
    }
    if matches!(mode, 2 | 4 | 6) {
        // This needs the console's fuse ID, so a PSP's own hashes of modes
        // 2, 4 and 6 can't be reproduced elsewhere. Like other PC tools we
        // use a made-up ID; PSPs and emulators accept such saves.
        hash = encrypted(&fuse_key(0), hash);
        hash = encrypted(key, hash);
    }
    if let Some(game_key) = game_key {
        xor16(&mut hash, game_key);
        hash = encrypted(key, hash);
    }
    hash
}

/// Remove the system layer of a save file.
pub fn decrypt_file(file: &[u8], params: u8, game_key: &[u8; 16]) -> Result<Vec<u8>, Error> {
    if file.len() < 16 {
        return Err(Error::new("PSP save file too short"));
    }
    let mode = mode_from_params(params, Some(game_key))?;
    let header: [u8; 16] = file[..16].try_into().unwrap();
    let len = file.len() - 16;
    let mut data = file[16..].to_vec();
    data.resize(len.next_multiple_of(16), 0);
    crypt_data(mode, &header, Some(game_key), &mut data);
    data.truncate(len);
    Ok(data)
}

/// Add the system layer to a save file. Returns the file and its hash for
/// `SAVEDATA_FILE_LIST`.
pub fn encrypt_file(
    data: &[u8],
    params: u8,
    game_key: &[u8; 16],
    header: [u8; 16],
) -> Result<(Vec<u8>, [u8; 16]), Error> {
    let mode = mode_from_params(params, Some(game_key))?;
    let mut file = header.to_vec();
    file.extend_from_slice(data);
    file.resize(16 + data.len().next_multiple_of(16), 0);
    crypt_data(mode, &header, Some(game_key), &mut file[16..]);
    let hash = sd_hash(mode, &file, Some(game_key));
    file.truncate(16 + data.len());
    Ok((file, hash))
}

/// A `PARAM.SFO` file.
pub struct Sfo(pub Vec<u8>);

impl Sfo {
    pub fn parse(data: Vec<u8>) -> Result<Sfo, Error> {
        if data.len() < 20 || &data[..8] != b"\0PSF\x01\x01\0\0" {
            return Err(Error::new("not a PARAM.SFO file"));
        }
        Ok(Sfo(data))
    }

    fn u32_at(&self, off: usize) -> usize {
        u32::from_le_bytes(self.0[off..off + 4].try_into().unwrap()) as usize
    }

    /// Range of a parameter's value.
    pub fn find(&self, name: &str) -> Result<std::ops::Range<usize>, Error> {
        let (keys, values, count) = (self.u32_at(8), self.u32_at(12), self.u32_at(16));
        for i in 0..count {
            let entry = 20 + 16 * i;
            let key = keys + u16::from_le_bytes([self.0[entry], self.0[entry + 1]]) as usize;
            let key_end = key + self.0[key..].iter().position(|&b| b == 0).unwrap_or(0);
            if &self.0[key..key_end] == name.as_bytes() {
                let start = values + self.u32_at(entry + 12);
                let end = start + self.u32_at(entry + 8);
                if end > self.0.len() {
                    break;
                }
                return Ok(start..end);
            }
        }
        Err(Error::new(format!("PARAM.SFO has no valid {name}")))
    }

    pub fn params(&self) -> Result<u8, Error> {
        Ok(self.0[self.find("SAVEDATA_PARAMS")?.start])
    }

    /// Store a file's hash in `SAVEDATA_FILE_LIST` and update the hashes in
    /// `SAVEDATA_PARAMS`, like Apollo Save Tool does.
    pub fn update(&mut self, file_name: &str, file_hash: &[u8; 16]) -> Result<(), Error> {
        let list = self.find("SAVEDATA_FILE_LIST")?;
        let entry = list
            .clone()
            .step_by(0x20)
            .take_while(|&e| e + 0x20 <= list.end)
            .find(|&e| {
                let name = &self.0[e..e + 0x0D];
                let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                &name[..end] == file_name.as_bytes()
            })
            .ok_or_else(|| Error::new(format!("{file_name} is not in PARAM.SFO")))?;
        self.0[entry + 0x0D..entry + 0x1D].copy_from_slice(file_hash);

        let params = self.find("SAVEDATA_PARAMS")?.start;
        let mode = self.0[params] >> 4;
        let check_bit = self.0[params] & 0xF;
        self.0[params..params + 0x80].fill(0);
        let put = |sfo: &mut Self, off: usize, hash_mode: u32| {
            let hash = sd_hash(hash_mode, &sfo.0, None);
            sfo.0[params + off..params + off + 16].copy_from_slice(&hash);
        };
        if mode & 0x4 != 0 {
            put(self, 0x20, 6);
            self.0[params] |= 0x41;
            put(self, 0x70, 5);
        } else if mode & 0x2 != 0 {
            put(self, 0x20, 4);
            self.0[params] |= 0x21;
            put(self, 0x70, 3);
        } else {
            put(self, 0x20, 2);
            self.0[params] |= 0x01;
        }
        if check_bit & 0x1 != 0 {
            put(self, 0x10, 1);
        }
        Ok(())
    }

    /// Check the hashes in `SAVEDATA_PARAMS` that don't depend on the
    /// console: the ones at 0x70 (modes 5 and 3) and 0x10 (mode 1). The one
    /// at 0x20 of modes 6 and 4 needs the fuse ID of the console that wrote
    /// it.
    pub fn check_params_hashes(&self) -> Result<(), Error> {
        let params = self.find("SAVEDATA_PARAMS")?.start;
        let flags = self.0[params];
        let mut copy = self.0.clone();
        copy[params + 0x10..params + 0x20].fill(0);
        let second = if flags >> 4 & 0x4 != 0 {
            Some(5)
        } else if flags >> 4 & 0x2 != 0 {
            Some(3)
        } else {
            None
        };
        if let Some(mode) = second {
            let stored = &copy[params + 0x70..params + 0x80].to_vec();
            copy[params + 0x70..params + 0x80].fill(0);
            if sd_hash(mode, &copy, None) != stored[..] {
                return Err(Error::new("PARAM.SFO: bad SAVEDATA_PARAMS hash at 0x70"));
            }
            copy[params + 0x70..params + 0x80].copy_from_slice(stored);
        }
        if flags & 0x1 != 0 && sd_hash(1, &copy, None) != self.0[params + 0x10..params + 0x20] {
            return Err(Error::new("PARAM.SFO: bad SAVEDATA_PARAMS hash at 0x10"));
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn rfc4493_cmac() {
        let key: [u8; 16] = hex("2b7e151628aed2a6abf7158809cf4f3c").try_into().unwrap();
        let msg = hex(concat!(
            "6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710"
        ));
        let cases = [
            (0, "bb1d6929e95937287fa37d129b756746"),
            (16, "070a16b46b4d4144f79bdd9dd04a287c"),
            (40, "dfa66747de9ae63030ca32611497c827"),
            (64, "51f0bebf7e3b9d92fc49741779363cfe"),
        ];
        for (len, mac) in cases {
            assert_eq!(cmac(&key, &msg[..len]).to_vec(), hex(mac), "{len}");
        }
    }

    #[test]
    fn modes_from_params() {
        let key = Some(&MHP2G_JP_KEY);
        assert_eq!(mode_from_params(0x41, key).unwrap(), 5);
        assert_eq!(mode_from_params(0x21, key).unwrap(), 3);
        assert_eq!(mode_from_params(0x41, None).unwrap(), 1);
        assert!(mode_from_params(0x01, key).is_err());
    }

    #[test]
    fn file_round_trip() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        for params in [0x41, 0x21] {
            let (file, hash) = encrypt_file(&data, params, &MHP2G_JP_KEY, [0xAB; 16]).unwrap();
            assert_eq!(file.len(), data.len() + 16);
            assert_ne!(file[16..32], data[..16]);
            assert_eq!(decrypt_file(&file, params, &MHP2G_JP_KEY).unwrap(), data);
            let (_, hash2) = encrypt_file(&data, params, &MHP2G_JP_KEY, [0xAC; 16]).unwrap();
            assert_ne!(hash, hash2);
        }
    }

    #[test]
    fn freedom_unite_key() {
        let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
        let (file, hash) = encrypt_file(&data, 0x41, &MHFU_KEY, [0xAB; 16]).unwrap();
        assert_eq!(decrypt_file(&file, 0x41, &MHFU_KEY).unwrap(), data);
        // The other game's key gives another file and hash.
        let (other, other_hash) = encrypt_file(&data, 0x41, &MHP2G_JP_KEY, [0xAB; 16]).unwrap();
        assert_ne!(file, other);
        assert_ne!(hash, other_hash);
        assert_ne!(decrypt_file(&file, 0x41, &MHP2G_JP_KEY).unwrap(), data);
    }

    pub fn synthetic_sfo() -> Vec<u8> {
        // Two parameters: SAVEDATA_FILE_LIST (0xC60 bytes) and
        // SAVEDATA_PARAMS (0x80 bytes).
        let keys = b"SAVEDATA_FILE_LIST\0SAVEDATA_PARAMS\0\0";
        let key_table = 20 + 2 * 16;
        let value_table = key_table + keys.len();
        let mut sfo = b"\0PSF\x01\x01\0\0".to_vec();
        for v in [key_table, value_table, 2] {
            sfo.extend_from_slice(&(v as u32).to_le_bytes());
        }
        for (key_off, len, off) in [(0u16, 0xC60u32, 0u32), (19, 0x80, 0xC60)] {
            sfo.extend_from_slice(&key_off.to_le_bytes());
            sfo.extend_from_slice(&4u16.to_le_bytes());
            sfo.extend_from_slice(&len.to_le_bytes());
            sfo.extend_from_slice(&len.to_le_bytes());
            sfo.extend_from_slice(&off.to_le_bytes());
        }
        sfo.extend_from_slice(keys);
        let mut values = vec![0; 0xC60 + 0x80];
        values[..11].copy_from_slice(b"MHP2NDG.BIN");
        values[0xC60] = 0x41;
        sfo.extend_from_slice(&values);
        sfo
    }

    #[test]
    fn sfo_update() {
        let mut sfo = Sfo::parse(synthetic_sfo()).unwrap();
        assert_eq!(sfo.params().unwrap(), 0x41);
        let list = sfo.find("SAVEDATA_FILE_LIST").unwrap().start;
        let params = sfo.find("SAVEDATA_PARAMS").unwrap().start;
        sfo.update("MHP2NDG.BIN", &[0x5A; 16]).unwrap();
        assert_eq!(sfo.0[list + 0x0D..list + 0x1D], [0x5A; 16]);
        assert_eq!(sfo.0[params], 0x41);
        assert_ne!(sfo.0[params + 0x20..params + 0x30], [0; 16]);
        assert_ne!(sfo.0[params + 0x70..params + 0x80], [0; 16]);
        // The low bit of 0x41 asks for the type 1 hash as well.
        assert_ne!(sfo.0[params + 0x10..params + 0x20], [0; 16]);
        sfo.check_params_hashes().unwrap();
        sfo.0[list] ^= 1;
        assert!(sfo.check_params_hashes().is_err());
        assert!(sfo.update("OTHER.BIN", &[0; 16]).is_err());
    }
}
