// SPDX-License-Identifier: GPL-3.0-or-later

//! Sealed app storage: every file the app writes is AES-256-GCM
//! encrypted under a key derived from the KeyOS app seed, with the file name as associated
//! data so files cannot be swapped. Layout: `NIPP` || 12-byte nonce || ciphertext+tag.
//!
//! A copied disk image or backup shows neither messages nor who you talk to.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroize;

use crate::{Error, Result};

const MAGIC: &[u8; 4] = b"NIPP";

fn storage_key(app_seed: &[u8; 32]) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(b"nip-ping/v1"), app_seed);
    let mut k = [0u8; 32];
    hk.expand(b"storage", &mut k).expect("32 bytes is a valid HKDF length");
    k
}

/// File-name tag of an identity (16 hex characters): keyed by the app seed, so a copied disk
/// shows which files belong together but not whose npub they are.
pub fn file_tag(app_seed: &[u8; 32], pubkey_hex: &str) -> String {
    let hk = Hkdf::<Sha256>::new(Some(b"nip-ping/v1"), app_seed);
    let mut t = [0u8; 8];
    let info = [b"file-name:".as_slice(), pubkey_hex.as_bytes()].concat();
    hk.expand(&info, &mut t).expect("8 bytes is a valid HKDF length");
    hex::encode(t)
}

pub fn seal_file(app_seed: &[u8; 32], name: &str, plain: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce).map_err(|_| Error::Rng)?;
    let mut key = storage_key(app_seed);
    let cipher = Aes256Gcm::new((&key).into());
    key.zeroize();
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: name.as_bytes() })
        .map_err(|_| Error::Vault("encrypt failed"))?;
    let mut out = Vec::with_capacity(4 + 12 + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

pub fn open_file(app_seed: &[u8; 32], name: &str, sealed: &[u8]) -> Result<Vec<u8>> {
    if sealed.len() < 4 + 12 + 16 || &sealed[..4] != MAGIC {
        return Err(Error::Vault("not a sealed file"));
    }
    let mut key = storage_key(app_seed);
    let cipher = Aes256Gcm::new((&key).into());
    key.zeroize();
    cipher
        .decrypt(Nonce::from_slice(&sealed[4..16]), Payload { msg: &sealed[16..], aad: name.as_bytes() })
        .map_err(|_| Error::Vault("authentication failed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_tamper_detection() {
        let seed = [5u8; 32];
        let plain = br#"{"text":"meet at noon"}"#;
        let sealed = seal_file(&seed, "inbox-0.json", plain).unwrap();
        assert_eq!(&sealed[..4], b"NIPP");
        assert!(!sealed.windows(4).any(|w| w == b"noon"), "plaintext leaked");
        assert_eq!(open_file(&seed, "inbox-0.json", &sealed).unwrap(), plain);
        let mut flipped = sealed.clone();
        let last = flipped.len() - 1;
        flipped[last] ^= 1;
        assert!(open_file(&seed, "inbox-0.json", &flipped).is_err());
        assert!(open_file(&[6u8; 32], "inbox-0.json", &sealed).is_err());
        assert!(open_file(&seed, "contacts-0.json", &sealed).is_err());
    }

    #[test]
    fn file_tags_hide_the_npub() {
        let pk = "7e7e9c42a91bfef19fa929e5fda1b72e0ebc1a4c1141673e2794234d86addf4e";
        let t = file_tag(&[5u8; 32], pk);
        assert_eq!(t.len(), 16);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!pk.contains(&t[..8]));
        assert_eq!(t, file_tag(&[5u8; 32], pk), "stable");
        assert_ne!(t, file_tag(&[6u8; 32], pk), "per device");
        assert_ne!(t, file_tag(&[5u8; 32], &pk.replace('7', "8")), "per key");
    }
}
