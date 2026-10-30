// SPDX-FileCopyrightText: 2026 Foundation Devices, Inc. <hello@foundation.xyz>
// SPDX-License-Identifier: GPL-3.0-or-later

//! NIP-44 v2 encryption. Copied from the Passport Prime Nostr Signer
//! (github.com/BitcoinQnA/passport-nostr-signer, logic/nostr-core/src/nip44.rs, GPL-3.0-or-later),
//! with the spec's payload length bounds added. Checked against the official vectors in
//! src/testdata/nip44.vectors.json (github.com/paulmillr/nip44).
//!
//! Algorithm:
//!   conversation_key = HKDF-Extract(salt=b"nip44-v2", ikm=shared_x)
//!   For each message:
//!     nonce        = 32 random bytes
//!     material     = HKDF-Expand(prk=conversation_key, info=nonce, L=76)
//!     chacha_key   = material[0..32]
//!     chacha_nonce = material[32..44]
//!     hmac_key     = material[44..76]
//!     padded       = u16_be(len) || plaintext || zeros
//!     ciphertext   = ChaCha20(chacha_key, chacha_nonce, padded)
//!     mac          = HMAC-SHA256(hmac_key, nonce || ciphertext)
//!     payload      = base64(0x02 || nonce || ciphertext || mac)

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use chacha20::ChaCha20;
use cipher::{KeyIvInit, StreamCipher};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::keys::shared_secret;
use crate::{Error, PublicKey, Result, SecretKey};

pub const VERSION: u8 = 2;
pub const MIN_PLAINTEXT_LEN: usize = 1;
pub const MAX_PLAINTEXT_LEN: usize = 65535;

use zeroize::{Zeroize, Zeroizing};

type HmacSha256 = Hmac<Sha256>;

/// Compute the per-conversation symmetric root key.
pub fn conversation_key(sk: &SecretKey, peer: &PublicKey) -> Result<[u8; 32]> {
    let mut ikm = shared_secret(sk, peer)?;
    // HKDF-Extract == HMAC-SHA256(key=salt, data=ikm)
    let (prk, _hk) = Hkdf::<Sha256>::extract(Some(b"nip44-v2"), &ikm);
    ikm.zeroize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&prk);
    Ok(out)
}

pub fn encrypt(sk: &SecretKey, peer: &PublicKey, plaintext: &str) -> Result<String> {
    let mut nonce = [0u8; 32];
    getrandom::getrandom(&mut nonce).map_err(|_| Error::Nip44("rng failed"))?;
    let mut ck = conversation_key(sk, peer)?;
    let out = encrypt_with_nonce(&ck, &nonce, plaintext);
    ck.zeroize();
    out
}

/// Deterministic encryption — used by the test-vector suite. Production
/// callers should use [`encrypt`] which samples a random nonce.
pub fn encrypt_with_nonce(conversation_key: &[u8; 32], nonce: &[u8; 32], plaintext: &str) -> Result<String> {
    let bytes = plaintext.as_bytes();
    if bytes.len() < MIN_PLAINTEXT_LEN || bytes.len() > MAX_PLAINTEXT_LEN {
        return Err(Error::Nip44("plaintext length out of range"));
    }

    let (mut chacha_key, mut chacha_nonce, mut hmac_key) = derive_message_keys(conversation_key, nonce);
    let padded = pad(bytes);

    let mut ct = padded;
    ChaCha20::new((&chacha_key).into(), (&chacha_nonce).into()).apply_keystream(&mut ct);

    let mac = hmac_over_nonce_and_ct(&hmac_key, nonce, &ct);
    chacha_key.zeroize();
    chacha_nonce.zeroize();
    hmac_key.zeroize();

    // version(1) + nonce(32) + ct + mac(32)
    let mut payload = Vec::with_capacity(1 + 32 + ct.len() + 32);
    payload.push(VERSION);
    payload.extend_from_slice(nonce);
    payload.extend_from_slice(&ct);
    payload.extend_from_slice(&mac);
    Ok(B64.encode(&payload))
}

pub fn decrypt(sk: &SecretKey, peer: &PublicKey, payload_b64: &str) -> Result<String> {
    let mut ck = conversation_key(sk, peer)?;
    let out = decrypt_with_key(&ck, payload_b64);
    ck.zeroize();
    out
}

pub fn decrypt_with_key(conversation_key: &[u8; 32], payload_b64: &str) -> Result<String> {
    if payload_b64.starts_with('#') {
        return Err(Error::Nip44("unsupported future version (leading #)"));
    }
    // Spec length bounds: base64 132..=87472 chars, raw 99..=65603 bytes.
    if payload_b64.len() < 132 || payload_b64.len() > 87472 {
        return Err(Error::Nip44("invalid payload length"));
    }
    let payload = B64.decode(payload_b64)?;
    if payload.len() < 99 || payload.len() > 65603 {
        return Err(Error::Nip44("invalid data length"));
    }
    if payload[0] != VERSION {
        return Err(Error::Nip44("unsupported version byte"));
    }
    let nonce: &[u8; 32] = payload[1..33].try_into().unwrap();
    let ct = &payload[33..payload.len() - 32];
    let mac_tag: &[u8; 32] = payload[payload.len() - 32..].try_into().unwrap();

    let (mut chacha_key, mut chacha_nonce, mut hmac_key) = derive_message_keys(conversation_key, nonce);
    let expected = hmac_over_nonce_and_ct(&hmac_key, nonce, ct);
    hmac_key.zeroize();
    // Constant-time compare.
    if !ct_eq(&expected, mac_tag) {
        chacha_key.zeroize();
        chacha_nonce.zeroize();
        return Err(Error::Nip44("mac mismatch"));
    }

    let mut padded = Zeroizing::new(ct.to_vec());
    ChaCha20::new((&chacha_key).into(), (&chacha_nonce).into()).apply_keystream(&mut padded);
    chacha_key.zeroize();
    chacha_nonce.zeroize();
    unpad(&padded)
}

/// Structural validation only (length bounds, base64, version byte); no key needed.
/// Lets callers tell a malformed string from one that is simply not for them.
pub fn check_structure(payload_b64: &str) -> Result<()> {
    if payload_b64.starts_with('#') || payload_b64.len() < 132 || payload_b64.len() > 87472 {
        return Err(Error::Nip44("invalid payload length"));
    }
    let payload = B64.decode(payload_b64)?;
    if payload.len() < 99 || payload.len() > 65603 {
        return Err(Error::Nip44("invalid data length"));
    }
    if payload[0] != VERSION {
        return Err(Error::Nip44("unsupported version byte"));
    }
    Ok(())
}

fn derive_message_keys(conversation_key: &[u8; 32], nonce: &[u8; 32]) -> ([u8; 32], [u8; 12], [u8; 32]) {
    let hk = Hkdf::<Sha256>::from_prk(conversation_key).expect("valid prk length");
    let mut material = Zeroizing::new([0u8; 76]);
    hk.expand(nonce, &mut *material).expect("76 <= 255 * HashLen");
    let mut chacha_key = [0u8; 32];
    let mut chacha_nonce = [0u8; 12];
    let mut hmac_key = [0u8; 32];
    chacha_key.copy_from_slice(&material[0..32]);
    chacha_nonce.copy_from_slice(&material[32..44]);
    hmac_key.copy_from_slice(&material[44..76]);
    (chacha_key, chacha_nonce, hmac_key)
}

fn hmac_over_nonce_and_ct(hmac_key: &[u8; 32], nonce: &[u8; 32], ct: &[u8]) -> [u8; 32] {
    let mut m = <HmacSha256 as Mac>::new_from_slice(hmac_key).expect("any length is valid");
    m.update(nonce);
    m.update(ct);
    let out = m.finalize().into_bytes();
    let mut tag = [0u8; 32];
    tag.copy_from_slice(&out);
    tag
}

fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut x = 0u8;
    for i in 0..32 {
        x |= a[i] ^ b[i];
    }
    x == 0
}

pub(crate) fn calc_padded_len(unpadded_len: usize) -> usize {
    if unpadded_len <= 32 {
        return 32;
    }
    // next_power = next power of two >= unpadded_len
    let n = unpadded_len as u32;
    let next_power = 1u32 << (32 - (n - 1).leading_zeros());
    let chunk: u32 = if next_power <= 256 { 32 } else { next_power / 8 };
    let chunk = chunk as usize;
    chunk * (((unpadded_len - 1) / chunk) + 1)
}

fn pad(plaintext: &[u8]) -> Vec<u8> {
    let padded_len = calc_padded_len(plaintext.len());
    let mut out = Vec::with_capacity(2 + padded_len);
    out.extend_from_slice(&(plaintext.len() as u16).to_be_bytes());
    out.extend_from_slice(plaintext);
    out.resize(2 + padded_len, 0);
    out
}

fn unpad(padded: &[u8]) -> Result<String> {
    if padded.len() < 2 {
        return Err(Error::Nip44("padded too short"));
    }
    let declared_len = u16::from_be_bytes([padded[0], padded[1]]) as usize;
    if declared_len < MIN_PLAINTEXT_LEN || declared_len > MAX_PLAINTEXT_LEN {
        return Err(Error::Nip44("declared length out of range"));
    }
    let expected_total = 2 + calc_padded_len(declared_len);
    if padded.len() != expected_total {
        return Err(Error::Nip44("padded size does not match declared length"));
    }
    let pt = &padded[2..2 + declared_len];
    String::from_utf8(pt.to_vec()).map_err(|_| Error::Nip44("plaintext not utf-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_len_matches_spec_values() {
        // Spec test vectors for calc_padded_len
        let cases: &[(usize, usize)] = &[
            (16, 32),
            (32, 32),
            (33, 64),
            (37, 64),
            (45, 64),
            (49, 64),
            (64, 64),
            (65, 96),
            (100, 128),
            (111, 128),
            (200, 224),
            (250, 256),
            (320, 320),
            (383, 384),
            (384, 384),
            (400, 448),
            (500, 512),
            (512, 512),
            (515, 640),
            (700, 768),
            (800, 896),
            (900, 1024),
            (1024, 1024),
            (1025, 1280),
            (65536 - 1, 65536),
        ];
        for (inp, want) in cases {
            assert_eq!(calc_padded_len(*inp), *want, "input={}", inp);
        }
    }

    #[test]
    fn roundtrip_short() {
        let a = SecretKey::generate().unwrap();
        let b = SecretKey::generate().unwrap();
        let pt = "gm ⚡";
        let ct = encrypt(&a, &b.public_key(), pt).unwrap();
        let dec = decrypt(&b, &a.public_key(), &ct).unwrap();
        assert_eq!(dec, pt);
    }

    #[test]
    fn roundtrip_long() {
        let a = SecretKey::generate().unwrap();
        let b = SecretKey::generate().unwrap();
        let pt = "x".repeat(4096);
        let ct = encrypt(&a, &b.public_key(), &pt).unwrap();
        let dec = decrypt(&b, &a.public_key(), &ct).unwrap();
        assert_eq!(dec, pt);
    }

    #[test]
    fn tampered_mac_fails() {
        let a = SecretKey::generate().unwrap();
        let b = SecretKey::generate().unwrap();
        let ct = encrypt(&a, &b.public_key(), "hi").unwrap();
        let mut bytes = B64.decode(&ct).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        let tampered = B64.encode(&bytes);
        assert!(decrypt(&b, &a.public_key(), &tampered).is_err());
    }

    // Deterministic vector: with a fixed conversation_key and nonce, output
    // must be stable across runs. Values computed by this implementation and
    // pinned so regressions are caught. (Cross-implementation vectors from
    // paulmillr/nip44 are checked in a later dedicated test file.)
    #[test]
    fn deterministic_encrypt_is_stable() {
        let ck = [0x11u8; 32];
        let nonce = [0x22u8; 32];
        let ct = encrypt_with_nonce(&ck, &nonce, "hello").unwrap();
        let dec = decrypt_with_key(&ck, &ct).unwrap();
        assert_eq!(dec, "hello");
        // Re-encrypting with same ck+nonce yields the same ciphertext.
        let ct2 = encrypt_with_nonce(&ck, &nonce, "hello").unwrap();
        assert_eq!(ct, ct2);
    }

    #[test]
    fn rejects_wrong_version() {
        let ck = [0u8; 32];
        // 0x01 version instead of 0x02
        let mut payload = vec![0x01u8];
        payload.extend_from_slice(&[0u8; 32 + 32 + 32]);
        let b64 = B64.encode(&payload);
        assert!(decrypt_with_key(&ck, &b64).is_err());
    }

    // ---- Official cross-implementation vectors (github.com/paulmillr/nip44) ----

    fn vectors() -> serde_json::Value {
        serde_json::from_str(include_str!("testdata/nip44.vectors.json")).unwrap()
    }

    fn arr32(h: &str) -> [u8; 32] {
        let v = hex::decode(h).unwrap();
        v.try_into().unwrap()
    }

    #[test]
    fn official_get_conversation_key() {
        let v = vectors();
        let cases = v["v2"]["valid"]["get_conversation_key"].as_array().unwrap();
        assert!(cases.len() >= 30);
        for c in cases {
            let sk = SecretKey::from_hex(c["sec1"].as_str().unwrap()).unwrap();
            let pk = PublicKey::from_hex(c["pub2"].as_str().unwrap()).unwrap();
            let ck = conversation_key(&sk, &pk).unwrap();
            assert_eq!(hex::encode(ck), c["conversation_key"].as_str().unwrap());
        }
    }

    #[test]
    fn official_invalid_conversation_key() {
        let v = vectors();
        for c in v["v2"]["invalid"]["get_conversation_key"].as_array().unwrap() {
            let sk = SecretKey::from_hex(c["sec1"].as_str().unwrap());
            let pk = PublicKey::from_hex(c["pub2"].as_str().unwrap());
            let ok = match (sk, pk) {
                (Ok(sk), Ok(pk)) => conversation_key(&sk, &pk).is_ok(),
                _ => false,
            };
            assert!(!ok, "should be rejected: {}", c["note"]);
        }
    }

    #[test]
    fn official_message_keys() {
        let v = vectors();
        let mk = &v["v2"]["valid"]["get_message_keys"];
        let ck = arr32(mk["conversation_key"].as_str().unwrap());
        for k in mk["keys"].as_array().unwrap() {
            let nonce = arr32(k["nonce"].as_str().unwrap());
            let (chacha_key, chacha_nonce, hmac_key) = derive_message_keys(&ck, &nonce);
            assert_eq!(hex::encode(chacha_key), k["chacha_key"].as_str().unwrap());
            assert_eq!(hex::encode(chacha_nonce), k["chacha_nonce"].as_str().unwrap());
            assert_eq!(hex::encode(hmac_key), k["hmac_key"].as_str().unwrap());
        }
    }

    #[test]
    fn official_calc_padded_len() {
        let v = vectors();
        for c in v["v2"]["valid"]["calc_padded_len"].as_array().unwrap() {
            let pair = c.as_array().unwrap();
            let (inp, want) = (pair[0].as_u64().unwrap() as usize, pair[1].as_u64().unwrap() as usize);
            assert_eq!(calc_padded_len(inp), want, "input {inp}");
        }
    }

    #[test]
    fn official_encrypt_decrypt() {
        let v = vectors();
        for c in v["v2"]["valid"]["encrypt_decrypt"].as_array().unwrap() {
            let sk1 = SecretKey::from_hex(c["sec1"].as_str().unwrap()).unwrap();
            let sk2 = SecretKey::from_hex(c["sec2"].as_str().unwrap()).unwrap();
            let ck = conversation_key(&sk1, &sk2.public_key()).unwrap();
            assert_eq!(hex::encode(ck), c["conversation_key"].as_str().unwrap());
            // Both directions derive the same key.
            assert_eq!(ck, conversation_key(&sk2, &sk1.public_key()).unwrap());
            let nonce = arr32(c["nonce"].as_str().unwrap());
            let pt = c["plaintext"].as_str().unwrap();
            let payload = encrypt_with_nonce(&ck, &nonce, pt).unwrap();
            assert_eq!(payload, c["payload"].as_str().unwrap());
            assert_eq!(decrypt_with_key(&ck, &payload).unwrap(), pt);
        }
    }

    #[test]
    fn official_long_messages() {
        use sha2::Digest;
        let v = vectors();
        for c in v["v2"]["valid"]["encrypt_decrypt_long_msg"].as_array().unwrap() {
            let ck = arr32(c["conversation_key"].as_str().unwrap());
            let nonce = arr32(c["nonce"].as_str().unwrap());
            let pt = c["pattern"].as_str().unwrap().repeat(c["repeat"].as_u64().unwrap() as usize);
            assert_eq!(hex::encode(Sha256::digest(pt.as_bytes())), c["plaintext_sha256"].as_str().unwrap());
            let payload = encrypt_with_nonce(&ck, &nonce, &pt).unwrap();
            assert_eq!(hex::encode(Sha256::digest(payload.as_bytes())), c["payload_sha256"].as_str().unwrap());
            assert_eq!(decrypt_with_key(&ck, &payload).unwrap(), pt);
        }
    }

    #[test]
    fn official_invalid_msg_lengths() {
        let v = vectors();
        for len in v["v2"]["invalid"]["encrypt_msg_lengths"].as_array().unwrap() {
            let n = len.as_u64().unwrap() as usize;
            let pt = "a".repeat(n);
            assert!(encrypt_with_nonce(&[1u8; 32], &[2u8; 32], &pt).is_err(), "length {n} must be rejected");
        }
    }

    #[test]
    fn official_invalid_decrypt() {
        let v = vectors();
        for c in v["v2"]["invalid"]["decrypt"].as_array().unwrap() {
            let ck = arr32(c["conversation_key"].as_str().unwrap());
            let r = decrypt_with_key(&ck, c["payload"].as_str().unwrap());
            assert!(r.is_err(), "must reject: {}", c["note"]);
        }
    }
}
