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
    cryptors: HashMap<MutVoidPtr, Cryptor>,
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
const kCCAlgorithmRC4: u32 = 4;
const kCCOptionPKCS7Padding: u32 = 1;
const kCCOptionECBMode: u32 = 2;

const AES_BLOCK: usize = 16;

#[derive(Clone)]
struct Rc4 {
    state: [u8; 256],
    i: u8,
    j: u8,
}
impl Rc4 {
    fn new(key: &[u8]) -> Self {
        let mut state = [0u8; 256];
        for (i, byte) in state.iter_mut().enumerate() {
            *byte = i as u8;
        }
        let mut j = 0u8;
        for i in 0..256 {
            j = j.wrapping_add(state[i]).wrapping_add(key[i % key.len()]);
            state.swap(i, j as usize);
        }
        Self { state, i: 0, j: 0 }
    }
    fn crypt(&mut self, input: &[u8]) -> Vec<u8> {
        input
            .iter()
            .map(|byte| {
                self.i = self.i.wrapping_add(1);
                self.j = self.j.wrapping_add(self.state[self.i as usize]);
                self.state.swap(self.i as usize, self.j as usize);
                let index = self.state[self.i as usize].wrapping_add(self.state[self.j as usize]);
                byte ^ self.state[index as usize]
            })
            .collect()
    }
}

fn rc4_crypt(key: &[u8], input: &[u8]) -> Vec<u8> {
    Rc4::new(key).crypt(input)
}

#[derive(Clone)]
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
        if pad == 0
            || pad > AES_BLOCK
            || pad > data.len()
            || data[data.len() - pad..]
                .iter()
                .any(|&byte| byte != pad as u8)
        {
            return Err(kCCDecodeError);
        }
        data.truncate(data.len() - pad);
    }
    Ok(data)
}

/// One-shot AES or RC4 encryption from CommonCrypto.
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
    if alg != kCCAlgorithmAES && alg != kCCAlgorithmRC4 {
        log!("TODO: CCCrypt() with algorithm {}", alg);
        return kCCUnimplemented;
    }
    if op != kCCEncrypt && op != kCCDecrypt {
        return kCCParamError;
    }
    if alg == kCCAlgorithmRC4 {
        if key.is_null()
            || !(1..=512).contains(&key_length)
            || options != 0
            || (data_in_length != 0 && (data_in.is_null() || data_out.is_null()))
        {
            return kCCParamError;
        }
        if !data_out_moved.is_null() {
            env.mem.write(data_out_moved, data_in_length);
        }
        if data_in_length > data_out_available {
            return kCCBufferTooSmall;
        }
        if data_in_length != 0 {
            let output = rc4_crypt(
                env.mem.bytes_at(key.cast(), key_length),
                env.mem.bytes_at(data_in.cast(), data_in_length),
            );
            env.mem
                .bytes_at_mut(data_out.cast(), data_in_length)
                .copy_from_slice(&output);
        }
        return kCCSuccess;
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

#[derive(Clone)]
struct Cryptor {
    cipher: Option<AesCipher>,
    rc4: Option<Rc4>,
    encrypt: bool,
    ecb: bool,
    padding: bool,
    chain: [u8; AES_BLOCK],
    buffered: Vec<u8>,
}

#[allow(clippy::too_many_arguments)] // Matches the guest's CommonCrypto ABI.
fn CCCryptorCreate(
    env: &mut Environment,
    op: u32,
    alg: u32,
    options: u32,
    key: ConstVoidPtr,
    key_len: u32,
    iv: ConstVoidPtr,
    result: MutPtr<MutVoidPtr>,
) -> i32 {
    if result.is_null() || key.is_null() || op > 1 || options & !3 != 0 {
        return kCCParamError;
    }
    if alg != kCCAlgorithmAES && alg != kCCAlgorithmRC4 {
        return kCCUnimplemented;
    }
    if (alg == kCCAlgorithmAES && ![16, 24, 32].contains(&key_len))
        || (alg == kCCAlgorithmRC4 && (!(1..=512).contains(&key_len) || options != 0))
    {
        return kCCParamError;
    }
    let key = env.mem.bytes_at(key.cast(), key_len);
    let cipher = if alg == kCCAlgorithmAES {
        AesCipher::new(key)
    } else {
        None
    };
    let rc4 = if alg == kCCAlgorithmRC4 {
        Some(Rc4::new(key))
    } else {
        None
    };
    let mut chain = [0; AES_BLOCK];
    if alg == kCCAlgorithmAES && !iv.is_null() {
        chain.copy_from_slice(env.mem.bytes_at(iv.cast(), AES_BLOCK as u32));
    }
    let state = Cryptor {
        cipher,
        rc4,
        encrypt: op == kCCEncrypt,
        ecb: options & 2 != 0,
        padding: options & 1 != 0,
        chain,
        buffered: Vec::new(),
    };
    let handle = env.mem.alloc(4);
    env.libc_state.crypto.cryptors.insert(handle, state);
    env.mem.write(result, handle);
    kCCSuccess
}

fn cryptor_blocks(state: &mut Cryptor, data: &[u8]) -> Vec<u8> {
    let output = aes_crypt(
        state.encrypt,
        state.cipher.as_ref().unwrap(),
        state.ecb,
        false,
        state.chain,
        data,
    )
    .unwrap();
    if !data.is_empty() && !state.ecb {
        let chain_data = if state.encrypt { &output } else { data };
        state
            .chain
            .copy_from_slice(&chain_data[chain_data.len() - AES_BLOCK..]);
    }
    output
}

fn CCCryptorUpdate(
    env: &mut Environment,
    handle: MutVoidPtr,
    input: ConstVoidPtr,
    input_len: u32,
    output: MutVoidPtr,
    output_len: u32,
    moved: MutPtr<u32>,
) -> i32 {
    if !moved.is_null() {
        env.mem.write(moved, 0);
    }
    if input_len != 0 && input.is_null() {
        return kCCParamError;
    }
    let Some(mut state) = env.libc_state.crypto.cryptors.get(&handle).cloned() else {
        return kCCParamError;
    };
    if let Some(rc4) = &mut state.rc4 {
        if input_len > output_len {
            return kCCBufferTooSmall;
        }
        if input_len != 0 && output.is_null() {
            return kCCParamError;
        }
        if input_len != 0 {
            let result = rc4.crypt(env.mem.bytes_at(input.cast(), input_len));
            env.mem
                .bytes_at_mut(output.cast(), input_len)
                .copy_from_slice(&result);
        }
        if !moved.is_null() {
            env.mem.write(moved, input_len);
        }
        env.libc_state.crypto.cryptors.insert(handle, state);
        return kCCSuccess;
    }
    if input_len != 0 {
        state
            .buffered
            .extend_from_slice(env.mem.bytes_at(input.cast(), input_len));
    }
    let hold = if !state.encrypt && state.padding {
        AES_BLOCK
    } else {
        0
    };
    let count = state.buffered.len().saturating_sub(hold) / AES_BLOCK * AES_BLOCK;
    if count > output_len as usize {
        return kCCBufferTooSmall;
    }
    if count != 0 && output.is_null() {
        return kCCParamError;
    }
    let data: Vec<u8> = state.buffered.drain(..count).collect();
    let result = cryptor_blocks(&mut state, &data);
    if count != 0 {
        env.mem
            .bytes_at_mut(output.cast(), count as u32)
            .copy_from_slice(&result);
    }
    if !moved.is_null() {
        env.mem.write(moved, count as u32);
    }
    env.libc_state.crypto.cryptors.insert(handle, state);
    kCCSuccess
}

fn CCCryptorFinal(
    env: &mut Environment,
    handle: MutVoidPtr,
    output: MutVoidPtr,
    output_len: u32,
    moved: MutPtr<u32>,
) -> i32 {
    if !moved.is_null() {
        env.mem.write(moved, 0);
    }
    let Some(mut state) = env.libc_state.crypto.cryptors.get(&handle).cloned() else {
        return kCCParamError;
    };
    if state.rc4.is_some() {
        return kCCSuccess;
    }
    let result = match aes_crypt(
        state.encrypt,
        state.cipher.as_ref().unwrap(),
        state.ecb,
        state.padding,
        state.chain,
        &state.buffered,
    ) {
        Ok(data) => data,
        Err(status) => return status,
    };
    if result.len() > output_len as usize {
        return kCCBufferTooSmall;
    }
    if !result.is_empty() && output.is_null() {
        return kCCParamError;
    }
    if !result.is_empty() {
        env.mem
            .bytes_at_mut(output.cast(), result.len() as u32)
            .copy_from_slice(&result);
    }
    if !moved.is_null() {
        env.mem.write(moved, result.len() as u32);
    }
    state.buffered.clear();
    env.libc_state.crypto.cryptors.insert(handle, state);
    kCCSuccess
}

fn CCCryptorGetOutputLength(
    env: &mut Environment,
    handle: MutVoidPtr,
    input_len: u32,
    final_: bool,
) -> u32 {
    let Some(state) = env.libc_state.crypto.cryptors.get(&handle) else {
        return 0;
    };
    if state.rc4.is_some() {
        return input_len;
    }
    let total = (state.buffered.len() as u64) + u64::from(input_len);
    let size = if final_ {
        if state.encrypt && state.padding {
            (total / 16 + 1) * 16
        } else {
            total.div_ceil(16) * 16
        }
    } else {
        let hold = if !state.encrypt && state.padding {
            16
        } else {
            0
        };
        total.saturating_sub(hold) / 16 * 16
    };
    size.min(u64::from(u32::MAX)) as u32
}
fn CCCryptorReset(env: &mut Environment, handle: MutVoidPtr, iv: ConstVoidPtr) -> i32 {
    let Some(state) = env.libc_state.crypto.cryptors.get(&handle) else {
        return kCCParamError;
    };
    if state.rc4.is_some() {
        return kCCUnimplemented;
    }
    let mut chain = [0; AES_BLOCK];
    if !iv.is_null() {
        chain.copy_from_slice(env.mem.bytes_at(iv.cast(), AES_BLOCK as u32));
    }
    let Some(state) = env.libc_state.crypto.cryptors.get_mut(&handle) else {
        return kCCParamError;
    };
    state.chain = chain;
    state.buffered.clear();
    kCCSuccess
}
fn CCCryptorRelease(env: &mut Environment, handle: MutVoidPtr) -> i32 {
    if handle.is_null() {
        return kCCSuccess;
    }
    if env.libc_state.crypto.cryptors.remove(&handle).is_none() {
        return kCCParamError;
    }
    env.mem.free(handle);
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

#[allow(clippy::too_many_arguments)] // Matches the guest's CommonCrypto ABI.
fn CCKeyDerivationPBKDF(
    env: &mut Environment,
    algorithm: u32,
    password: ConstVoidPtr,
    password_len: u32,
    salt: ConstVoidPtr,
    salt_len: u32,
    prf: u32,
    rounds: u32,
    output: MutVoidPtr,
    output_len: u32,
) -> i32 {
    if algorithm != 2
        || rounds == 0
        || output.is_null()
        || output_len == 0
        || (password_len != 0 && password.is_null())
        || (salt_len != 0 && salt.is_null())
    {
        return kCCParamError;
    }
    let mac: fn(&[u8], &[u8]) -> Vec<u8> = match prf {
        1 => hmac::<Sha1>,
        2 => hmac::<sha2::Sha224>,
        3 => hmac::<sha2::Sha256>,
        4 => hmac::<sha2::Sha384>,
        5 => hmac::<sha2::Sha512>,
        _ => return kCCParamError,
    };
    let password = if password_len == 0 {
        Vec::new()
    } else {
        env.mem.bytes_at(password.cast(), password_len).to_vec()
    };
    let mut salt = if salt_len == 0 {
        Vec::new()
    } else {
        env.mem.bytes_at(salt.cast(), salt_len).to_vec()
    };
    let mut derived = Vec::with_capacity(output_len as usize);
    let mut block: u32 = 1;
    let salt_size = salt.len();
    while derived.len() < output_len as usize {
        salt.truncate(salt_size);
        salt.extend_from_slice(&block.to_be_bytes());
        let mut u = mac(&password, &salt);
        let mut result = u.clone();
        for _ in 1..rounds {
            u = mac(&password, &u);
            for (out, value) in result.iter_mut().zip(&u) {
                *out ^= *value;
            }
        }
        let remaining = output_len as usize - derived.len();
        derived.extend_from_slice(&result[..remaining.min(result.len())]);
        block += 1;
    }
    env.mem
        .bytes_at_mut(output.cast(), output_len)
        .copy_from_slice(&derived);
    kCCSuccess
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
    export_c_func!(CCCryptorCreate(_, _, _, _, _, _, _)),
    export_c_func!(CCCryptorUpdate(_, _, _, _, _, _)),
    export_c_func!(CCCryptorFinal(_, _, _, _)),
    export_c_func!(CCCryptorGetOutputLength(_, _, _)),
    export_c_func!(CCCryptorReset(_, _)),
    export_c_func!(CCCryptorRelease(_)),
    export_c_func!(CCKeyDerivationPBKDF(_, _, _, _, _, _, _, _, _)),
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

    #[test]
    fn rc4_known_answer_and_round_trip() {
        let ciphertext = rc4_crypt(b"Key", b"Plaintext");
        assert_eq!(
            ciphertext,
            [0xbb, 0xf3, 0x16, 0xe8, 0xd9, 0x40, 0xaf, 0x0a, 0xd3]
        );
        assert_eq!(rc4_crypt(b"Key", &ciphertext), b"Plaintext");
    }

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
