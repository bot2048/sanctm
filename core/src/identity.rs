// SPDX-License-Identifier: GPL-3.0-or-later

//! The Passport's Nostr identity.
//!
//! Derived, never stored: the KeyOS app seed (32 bytes, per app, from the device seed) is
//! read as BIP-39 entropy, and the key is the NIP-06 path `m/44'/1237'/<account>'/0/0`.
//! Restore the Passport from its seed words, reinstall this app (same app id), and the same
//! npub comes back. The secret key never leaves the device.
//!
//! An imported `nsec` (for people who already have a Nostr identity) is the one exception:
//! it is kept in the sealed vault, encrypted under a key from the same app seed.

use bip39::Mnemonic;
use hmac::{Hmac, Mac};
use k256::elliptic_curve::generic_array::GenericArray;
use k256::elliptic_curve::scalar::ScalarPrimitive;
use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::{ProjectivePoint, PublicKey as K256Pub, Scalar};
use sha2::Sha512;
use zeroize::Zeroize;

use crate::{nip19, Error, PublicKey, Result, SecretKey};

type HmacSha512 = Hmac<Sha512>;
const HARDENED: u32 = 0x8000_0000;
const NOSTR_COIN: u32 = 1237;

pub struct Identity {
    pub sk: SecretKey,
    pub pk: PublicKey,
}

impl Identity {
    pub fn from_secret(sk: SecretKey) -> Self {
        let pk = sk.public_key();
        Self { sk, pk }
    }

    pub fn npub(&self) -> String { nip19::encode_npub(&self.pk) }
}

/// NIP-06 key for `account`, rooted in the KeyOS app seed.
pub fn derive_from_app_seed(app_seed: &[u8; 32], account: u32) -> Result<Identity> {
    let mnemonic = Mnemonic::from_entropy(app_seed).map_err(|_| Error::Nip06("bad entropy"))?;
    let mut seed = mnemonic.to_seed("");
    let out = derive_from_bip39_seed(&seed, account);
    seed.zeroize();
    out.map(Identity::from_secret)
}

/// NIP-06 from a BIP-39 phrase (used by the tests against the NIP-06 vectors).
pub fn derive_from_mnemonic(phrase: &str, passphrase: &str, account: u32) -> Result<SecretKey> {
    let m = Mnemonic::parse(phrase).map_err(|_| Error::Nip06("invalid mnemonic"))?;
    let mut seed = m.to_seed(passphrase);
    let out = derive_from_bip39_seed(&seed, account);
    seed.zeroize();
    out
}

fn derive_from_bip39_seed(seed: &[u8; 64], account: u32) -> Result<SecretKey> {
    let mut mac = <HmacSha512 as Mac>::new_from_slice(b"Bitcoin seed").map_err(|_| Error::Nip06("hmac"))?;
    mac.update(seed);
    let i = mac.finalize().into_bytes();
    let (mut k, mut c) = ([0u8; 32], [0u8; 32]);
    k.copy_from_slice(&i[..32]);
    c.copy_from_slice(&i[32..]);
    scalar(&k)?;
    for index in [44 | HARDENED, NOSTR_COIN | HARDENED, account | HARDENED, 0, 0] {
        let (nk, nc) = child(&k, &c, index)?;
        k.zeroize();
        c.zeroize();
        (k, c) = (nk, nc);
    }
    c.zeroize();
    let sk = SecretKey::from_bytes(k);
    k.zeroize();
    sk
}

fn child(k: &[u8; 32], c: &[u8; 32], index: u32) -> Result<([u8; 32], [u8; 32])> {
    let mut mac = <HmacSha512 as Mac>::new_from_slice(c).map_err(|_| Error::Nip06("hmac"))?;
    let parent = scalar(k)?;
    if index >= HARDENED {
        mac.update(&[0u8]);
        mac.update(k);
    } else {
        let point = ProjectivePoint::GENERATOR * parent;
        let pk = K256Pub::from_affine(point.into()).map_err(|_| Error::Nip06("bad point"))?;
        mac.update(pk.to_encoded_point(true).as_bytes());
    }
    mac.update(&index.to_be_bytes());
    let i = mac.finalize().into_bytes();
    let mut il = [0u8; 32];
    il.copy_from_slice(&i[..32]);
    let tweak: Scalar = Option::<ScalarPrimitive<k256::Secp256k1>>::from(ScalarPrimitive::from_bytes(GenericArray::from_slice(&il)))
        .ok_or(Error::Nip06("IL out of range"))?
        .into();
    il.zeroize();
    let child = tweak + parent;
    if bool::from(child.is_zero()) {
        return Err(Error::Nip06("derived zero key"));
    }
    let mut nc = [0u8; 32];
    nc.copy_from_slice(&i[32..]);
    Ok((child.to_bytes().into(), nc))
}

fn scalar(bytes: &[u8; 32]) -> Result<Scalar> {
    let s: Scalar = Option::<ScalarPrimitive<k256::Secp256k1>>::from(ScalarPrimitive::from_bytes(GenericArray::from_slice(bytes)))
        .ok_or(Error::Nip06("scalar out of range"))?
        .into();
    if bool::from(s.is_zero()) {
        return Err(Error::Nip06("zero scalar"));
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    // NIP-06 official vectors.
    #[test]
    fn nip06_vectors() {
        let sk = derive_from_mnemonic("leader monkey parrot ring guide accident before fence cannon height naive bean", "", 0).unwrap();
        assert_eq!(hex::encode(sk.as_bytes()), "7f7ff03d123792d6ac594bfa67bf6d0c0ab55b6b1fdb6249303fe861f1ccba9a");
        let sk = derive_from_mnemonic(
            "what bleak badge arrange retreat wolf trade produce cricket blur garlic valid proud rude strong choose busy staff weather area salt hollow arm fade",
            "",
            0,
        )
        .unwrap();
        assert_eq!(hex::encode(sk.as_bytes()), "c15d739894c81a2fcfd3a2df85a0d2c0dbc47a280d092799f144d73d7ae78add");
    }

    #[test]
    fn app_seed_identity_is_stable_and_account_separated() {
        let seed = [7u8; 32];
        let a = derive_from_app_seed(&seed, 0).unwrap();
        assert_eq!(a.pk, derive_from_app_seed(&seed, 0).unwrap().pk);
        assert_ne!(a.pk, derive_from_app_seed(&seed, 1).unwrap().pk);
        assert_ne!(a.pk, derive_from_app_seed(&[8u8; 32], 0).unwrap().pk);
        // Same as running NIP-06 on the 24 words the app seed encodes.
        let words = Mnemonic::from_entropy(&seed).unwrap().to_string();
        assert_eq!(derive_from_mnemonic(&words, "", 0).unwrap().public_key(), a.pk);
        assert!(a.npub().starts_with("npub1"));
    }
}
