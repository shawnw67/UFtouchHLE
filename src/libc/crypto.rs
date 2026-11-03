/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! CommonCrypto and friends
#![allow(non_upper_case_globals)] // Names from the CommonCrypto headers.

use crate::dyld::FunctionExports;
use crate::mem::{ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr};
use crate::{export_c_func, Environment};
use aes::cipher::{generic_array::GenericArray, BlockDecrypt, BlockEncrypt, KeyInit};
use digest::Digest;
use hmac::{Hmac, Mac};
use md5::Md5;
use sha1::Sha1;
use std::collections::HashMap;

// TODO: struct definition
#[allow(non_camel_case_types)]
struct CC_MD5_CTX {}

#[derive(Default)]
pub struct State {
    md5_contexts: HashMap<MutPtr<CC_MD5_CTX>, Md5>,
    sha_contexts: HashMap<MutVoidPtr, ShaState>,
}
impl State {
    fn get_mut(env: &mut Environment) -> &mut Self {
        &mut env.libc_state.crypto
    }
}

fn CC_MD5_Init(env: &mut Environment, ctx: MutPtr<CC_MD5_CTX>) -> i32 {
    log_once!("Warning: CC_MD5_Init doesn't update side effects! (internal changes to CC_MD5_CTX are not done)");
    assert!(!State::get_mut(env).md5_contexts.contains_key(&ctx));
    State::get_mut(env).md5_contexts.insert(ctx, Md5::new());
    1 // success
}

fn CC_MD5_Update(
    env: &mut Environment,
    ctx: MutPtr<CC_MD5_CTX>,
    data: ConstVoidPtr,
    len: u32,
) -> i32 {
    log_once!("Warning: CC_MD5_Update doesn't update side effects! (internal changes to CC_MD5_CTX are not done)");
    let hasher = env.libc_state.crypto.md5_contexts.get_mut(&ctx).unwrap();
    hasher.update(env.mem.bytes_at(data.cast(), len));
    1 // success
}

fn CC_MD5_Final(env: &mut Environment, md: MutPtr<u8>, ctx: MutPtr<CC_MD5_CTX>) -> i32 {
    log_once!("Warning: CC_MD5_Final doesn't update side effects! (internal changes to CC_MD5_CTX are not done)");
    let hasher = State::get_mut(env).md5_contexts.remove(&ctx).unwrap();
    let digest = hasher.finalize();
    env.mem.bytes_at_mut(md, 16).copy_from_slice(&digest[..]);
    1 // success
}

fn CC_MD5(env: &mut Environment, data: ConstVoidPtr, len: u32, md: MutPtr<u8>) -> MutPtr<u8> {
    let mut hasher = Md5::new();
    hasher.update(env.mem.bytes_at(data.cast(), len));
    let digest = hasher.finalize();
    env.mem.bytes_at_mut(md, 16).copy_from_slice(&digest[..]);
    md
}

fn CC_SHA1(env: &mut Environment, data: ConstVoidPtr, len: u32, md: MutPtr<u8>) -> MutPtr<u8> {
    CC_SHA1_once(env, data, len, md)
}

// Like the existing MD5 implementation, incremental contexts are stored per
// emulated environment. Copying an opaque guest context is not yet supported.
enum ShaState {
    Sha1(Sha1),
    Sha224(sha2::Sha224),
    Sha256(sha2::Sha256),
    Sha384(sha2::Sha384),
    Sha512(sha2::Sha512),
}

macro_rules! sha_functions {
    ($once:ident, $init:ident, $update:ident, $final:ident, $variant:ident, $algorithm:ty) => {
        fn $once(
            env: &mut Environment,
            data: ConstVoidPtr,
            len: u32,
            output: MutPtr<u8>,
        ) -> MutPtr<u8> {
            if output.is_null() || (len != 0 && data.is_null()) {
                return MutPtr::null();
            }
            let mut hash = <$algorithm>::new();
            if len != 0 {
                hash.update(env.mem.bytes_at(data.cast(), len));
            }
            let digest = hash.finalize();
            env.mem
                .bytes_at_mut(output, digest.len() as u32)
                .copy_from_slice(&digest);
            output
        }
        fn $init(env: &mut Environment, ctx: MutVoidPtr) -> i32 {
            if ctx.is_null() {
                return 0;
            }
            env.libc_state
                .crypto
                .sha_contexts
                .insert(ctx, ShaState::$variant(<$algorithm>::new()));
            1
        }
        fn $update(env: &mut Environment, ctx: MutVoidPtr, data: ConstVoidPtr, len: u32) -> i32 {
            if len != 0 && data.is_null() {
                return 0;
            }
            let Some(ShaState::$variant(hash)) = env.libc_state.crypto.sha_contexts.get_mut(&ctx)
            else {
                return 0;
            };
            if len != 0 {
                hash.update(env.mem.bytes_at(data.cast(), len));
            }
            1
        }
        fn $final(env: &mut Environment, output: MutPtr<u8>, ctx: MutVoidPtr) -> i32 {
            if output.is_null() {
                return 0;
            }
            let Some(ShaState::$variant(hash)) = env.libc_state.crypto.sha_contexts.remove(&ctx)
            else {
                return 0;
            };
            let digest = hash.finalize();
            env.mem
                .bytes_at_mut(output, digest.len() as u32)
                .copy_from_slice(&digest);
            1
        }
    };
}
sha_functions!(
    CC_SHA1_once,
    CC_SHA1_Init,
    CC_SHA1_Update,
    CC_SHA1_Final,
    Sha1,
    Sha1
);
sha_functions!(
    CC_SHA224,
    CC_SHA224_Init,
    CC_SHA224_Update,
    CC_SHA224_Final,
    Sha224,
    sha2::Sha224
);
sha_functions!(
    CC_SHA256,
    CC_SHA256_Init,
    CC_SHA256_Update,
    CC_SHA256_Final,
    Sha256,
    sha2::Sha256
);
sha_functions!(
    CC_SHA384,
    CC_SHA384_Init,
    CC_SHA384_Update,
    CC_SHA384_Final,
    Sha384,
    sha2::Sha384
);
sha_functions!(
    CC_SHA512,
    CC_SHA512_Init,
    CC_SHA512_Update,
    CC_SHA512_Final,
    Sha512,
    sha2::Sha512
);

type CCCryptorStatus = i32;
const kCCSuccess: CCCryptorStatus = 0;
const kCCParamError: CCCryptorStatus = -4300;
const kCCBufferTooSmall: CCCryptorStatus = -4301;
const kCCAlignmentError: CCCryptorStatus = -4303;
const kCCDecodeError: CCCryptorStatus = -4304;
const kCCUnimplemented: CCCryptorStatus = -4305;

const kCCEncrypt: u32 = 0;
const kCCDecrypt: u32 = 1;
const kCCAlgorithmAES: u32 = 0;
const kCCOptionPKCS7Padding: u32 = 1;
const kCCOptionECBMode: u32 = 2;

const AES_BLOCK: usize = 16;

enum AesCipher {
    Aes128(aes::Aes128),
    Aes192(aes::Aes192),
    Aes256(aes::Aes256),
}
impl AesCipher {
    fn new(key: &[u8]) -> Option<Self> {
        match key.len() {
            16 => Some(Self::Aes128(aes::Aes128::new_from_slice(key).unwrap())),
            24 => Some(Self::Aes192(aes::Aes192::new_from_slice(key).unwrap())),
            32 => Some(Self::Aes256(aes::Aes256::new_from_slice(key).unwrap())),
            _ => None,
        }
    }
    fn encrypt(&self, block: &mut [u8]) {
        let block = GenericArray::from_mut_slice(block);
        match self {
            Self::Aes128(c) => c.encrypt_block(block),
            Self::Aes192(c) => c.encrypt_block(block),
            Self::Aes256(c) => c.encrypt_block(block),
        }
    }
    fn decrypt(&self, block: &mut [u8]) {
        let block = GenericArray::from_mut_slice(block);
        match self {
            Self::Aes128(c) => c.decrypt_block(block),
            Self::Aes192(c) => c.decrypt_block(block),
            Self::Aes256(c) => c.decrypt_block(block),
        }
    }
}

/// AES encryption or decryption in CBC mode (or ECB mode), optionally with
/// PKCS#7 padding. Returns the output or a CCCryptorStatus error.
fn aes_crypt(
    encrypt: bool,
    cipher: &AesCipher,
    ecb: bool,
    padding: bool,
    iv: [u8; AES_BLOCK],
    input: &[u8],
) -> Result<Vec<u8>, CCCryptorStatus> {
    let mut data = input.to_vec();
    if encrypt && padding {
        let pad = AES_BLOCK - data.len() % AES_BLOCK;
        data.resize(data.len() + pad, pad as u8);
    }
    if !data.len().is_multiple_of(AES_BLOCK) {
        return Err(kCCAlignmentError);
    }
    let mut chain = iv;
    for block in data.as_chunks_mut::<AES_BLOCK>().0 {
        if encrypt {
            if !ecb {
                for (b, c) in block.iter_mut().zip(chain.iter()) {
                    *b ^= c;
                }
            }
            cipher.encrypt(block);
            chain.copy_from_slice(block);
        } else {
            let ciphertext: [u8; AES_BLOCK] = *block;
            cipher.decrypt(block);
            if !ecb {
                for (b, c) in block.iter_mut().zip(chain.iter()) {
                    *b ^= c;
                }
            }
            chain = ciphertext;
        }
    }
    if !encrypt && padding {
        let pad = *data.last().ok_or(kCCDecodeError)? as usize;
        if pad == 0 || pad > AES_BLOCK || pad > data.len() {
            return Err(kCCDecodeError);
        }
        data.truncate(data.len() - pad);
    }
    Ok(data)
}

/// One-shot symmetric encryption from CommonCrypto. Only AES is supported.
#[allow(clippy::too_many_arguments)]
fn CCCrypt(
    env: &mut Environment,
    op: u32,
    alg: u32,
    options: u32,
    key: ConstVoidPtr,
    key_length: GuestUSize,
    iv: ConstVoidPtr,
    data_in: ConstVoidPtr,
    data_in_length: GuestUSize,
    data_out: MutVoidPtr,
    data_out_available: GuestUSize,
    data_out_moved: MutPtr<GuestUSize>,
) -> CCCryptorStatus {
    if alg != kCCAlgorithmAES {
        log!("TODO: CCCrypt() with algorithm {}", alg);
        return kCCUnimplemented;
    }
    if op != kCCEncrypt && op != kCCDecrypt {
        return kCCParamError;
    }
    // ---- MHP2G save analysis instrumentation ----
    if crate::save_research::enabled() && (data_in_length >= 1_000_000 || key_length >= 8) {
        log!("[CCCrypt] op={} alg={} options={} keyLen={} iv={:#x} dataIn={:#x} dataInLen={} dataOut={:#x} dataOutAvail={}",
             op, alg, options, key_length, iv.to_bits(), data_in.to_bits(), data_in_length, data_out.to_bits(), data_out_available);
        let klen: GuestUSize = key_length.min(64);
        log!("[CCCrypt] key={:02x?}", env.mem.bytes_at(key.cast(), klen));
        if !iv.is_null() {
            log!(
                "[CCCrypt] iv={:02x?}",
                env.mem.bytes_at(iv.cast(), AES_BLOCK as GuestUSize)
            );
        }
        if data_in_length >= 1_000_000 {
            let plain = env.mem.bytes_at(data_in.cast(), data_in_length).to_vec();
            crate::save_research::write("cccrypt_plain.bin", &plain);
            log!("[CCCrypt] plaintext dumped {} bytes", plain.len());
            // dump ±4MB around data_in to find the plaintext source buffer
            const AROUND: u32 = 4 * 1024 * 1024;
            let start = data_in.to_bits().saturating_sub(AROUND);
            let len = (AROUND * 2) as GuestUSize;
            let region = env
                .mem
                .bytes_at(ConstVoidPtr::from_bits(start).cast(), len)
                .to_vec();
            crate::save_research::write("cccrypt_around.bin", &region);
            log!(
                "[CCCrypt] around dump {} bytes @ {:#x}",
                region.len(),
                start
            );
            // dump the HaXe heap region 0x02000000..0x05000000 (plaintext
            // ByteArray candidate)
            let heap = env
                .mem
                .bytes_at(ConstVoidPtr::from_bits(0x02000000).cast(), 0x03000000)
                .to_vec();
            crate::save_research::write("cccrypt_heap.bin", &heap);
            log!("[CCCrypt] heap dump {} bytes @ 0x02000000", heap.len());
        }
        if op == kCCDecrypt && data_in_length >= 1_000_000 {
            // dump ±4MB around data_out (decryption output buffer)
            const AROUND2: u32 = 4 * 1024 * 1024;
            let start2 = data_out.to_bits().saturating_sub(AROUND2);
            let len2 = (AROUND2 * 2) as GuestUSize;
            let region2 = env
                .mem
                .bytes_at(ConstVoidPtr::from_bits(start2).cast(), len2)
                .to_vec();
            crate::save_research::write("cccrypt_decrypt_around.bin", &region2);
            log!(
                "[CCCrypt] decrypt-around dump {} bytes @ {:#x}",
                region2.len(),
                start2
            );
        }
    }
    // ---- end instrumentation ----
    let Some(cipher) = AesCipher::new(env.mem.bytes_at(key.cast(), key_length)) else {
        return kCCParamError;
    };
    let mut iv_bytes = [0u8; AES_BLOCK];
    if !iv.is_null() {
        iv_bytes.copy_from_slice(env.mem.bytes_at(iv.cast(), AES_BLOCK as GuestUSize));
    }
    let input = env.mem.bytes_at(data_in.cast(), data_in_length).to_vec();
    let result = aes_crypt(
        op == kCCEncrypt,
        &cipher,
        options & kCCOptionECBMode != 0,
        options & kCCOptionPKCS7Padding != 0,
        iv_bytes,
        &input,
    );
    let output = match result {
        Ok(output) => output,
        Err(status) => return status,
    };
    let output_len: GuestUSize = output.len().try_into().unwrap();
    if !data_out_moved.is_null() {
        env.mem.write(data_out_moved, output_len);
    }
    if output_len > data_out_available {
        return kCCBufferTooSmall;
    }
    env.mem
        .bytes_at_mut(data_out.cast(), output_len)
        .copy_from_slice(&output);
    kCCSuccess
}

fn hmac<D>(key: &[u8], data: &[u8]) -> Vec<u8>
where
    D: digest::core_api::CoreProxy,
    D::Core: digest::HashMarker
        + digest::core_api::UpdateCore
        + digest::core_api::FixedOutputCore
        + digest::core_api::BufferKindUser<BufferKind = digest::block_buffer::Eager>
        + Default
        + Clone,
    <D::Core as digest::core_api::BlockSizeUser>::BlockSize:
        digest::typenum::IsLess<digest::typenum::U256>,
    digest::typenum::Le<
        <D::Core as digest::core_api::BlockSizeUser>::BlockSize,
        digest::typenum::U256,
    >: digest::typenum::NonZero,
{
    let mut mac = <Hmac<D> as Mac>::new_from_slice(key).unwrap();
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// One-shot HMAC from CommonCrypto.
fn CCHmac(
    env: &mut Environment,
    algorithm: u32,
    key: ConstVoidPtr,
    key_length: GuestUSize,
    data: ConstVoidPtr,
    data_length: GuestUSize,
    mac_out: MutVoidPtr,
) {
    let key = env.mem.bytes_at(key.cast(), key_length).to_vec();
    let data = env.mem.bytes_at(data.cast(), data_length).to_vec();
    let mac = match algorithm {
        0 => hmac::<Sha1>(&key, &data),
        1 => hmac::<Md5>(&key, &data),
        2 => hmac::<sha2::Sha256>(&key, &data),
        3 => hmac::<sha2::Sha384>(&key, &data),
        4 => hmac::<sha2::Sha512>(&key, &data),
        5 => hmac::<sha2::Sha224>(&key, &data),
        _ => panic!("Unknown CCHmac algorithm {algorithm}"),
    };
    let len: GuestUSize = mac.len().try_into().unwrap();
    env.mem
        .bytes_at_mut(mac_out.cast(), len)
        .copy_from_slice(&mac);
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(CCCrypt(_, _, _, _, _, _, _, _, _, _, _)),
    export_c_func!(CCHmac(_, _, _, _, _, _)),
    export_c_func!(CC_MD5_Init(_)),
    export_c_func!(CC_MD5_Update(_, _, _)),
    export_c_func!(CC_MD5_Final(_, _)),
    export_c_func!(CC_MD5(_, _, _)),
    export_c_func!(CC_SHA1(_, _, _)),
    export_c_func!(CC_SHA1_Init(_)),
    export_c_func!(CC_SHA1_Update(_, _, _)),
    export_c_func!(CC_SHA1_Final(_, _)),
    export_c_func!(CC_SHA224(_, _, _)),
    export_c_func!(CC_SHA224_Init(_)),
    export_c_func!(CC_SHA224_Update(_, _, _)),
    export_c_func!(CC_SHA224_Final(_, _)),
    export_c_func!(CC_SHA256(_, _, _)),
    export_c_func!(CC_SHA256_Init(_)),
    export_c_func!(CC_SHA256_Update(_, _, _)),
    export_c_func!(CC_SHA256_Final(_, _)),
    export_c_func!(CC_SHA384(_, _, _)),
    export_c_func!(CC_SHA384_Init(_)),
    export_c_func!(CC_SHA384_Update(_, _, _)),
    export_c_func!(CC_SHA384_Final(_, _)),
    export_c_func!(CC_SHA512(_, _, _)),
    export_c_func!(CC_SHA512_Init(_)),
    export_c_func!(CC_SHA512_Update(_, _, _)),
    export_c_func!(CC_SHA512_Final(_, _)),
];

#[cfg(test)]
mod tests {
    use super::*;

    // FIPS-197 appendix C.1 (one block, so CBC with a zero IV is the same).
    #[test]
    fn aes_128_known_answer() {
        let key: Vec<u8> = (0u8..16).collect();
        let plaintext = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        let expected = [
            0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4,
            0xc5, 0x5a,
        ];
        let cipher = AesCipher::new(&key).unwrap();
        let out = aes_crypt(true, &cipher, false, false, [0; 16], &plaintext).unwrap();
        assert_eq!(out, expected);
        let back = aes_crypt(false, &cipher, false, false, [0; 16], &out).unwrap();
        assert_eq!(back, plaintext);
    }

    #[test]
    fn cbc_pkcs7_round_trip() {
        let cipher = AesCipher::new(&[7u8; 32]).unwrap();
        let data = b"Monster Hunter save data, 37 bytes!!";
        let encrypted = aes_crypt(true, &cipher, false, true, [3; 16], data).unwrap();
        assert_eq!(encrypted.len(), 48);
        let decrypted = aes_crypt(false, &cipher, false, true, [3; 16], &encrypted).unwrap();
        assert_eq!(decrypted, data);
    }
}
