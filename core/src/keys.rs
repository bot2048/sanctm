// SPDX-FileCopyrightText: 2026 Foundation Devices, Inc. <hello@foundation.xyz>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Key types from the Passport Prime Nostr Signer (logic/nostr-core/src/keys.rs,
// GPL-3.0-or-later), with x-only ECDH added.

use core::fmt;

use k256::elliptic_curve::sec1::ToEncodedPoint;
use k256::schnorr::{SigningKey, VerifyingKey};
use k256::{AffinePoint, ProjectivePoint, PublicKey as K256Pub, Scalar};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{Error, Result};

/// A secp256k1 secret scalar. Zeroised on drop, never printed.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SecretKey([u8; 32]);

/// A BIP-340 x-only public key.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PublicKey([u8; 32]);

impl SecretKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self> {
        SigningKey::from_bytes(&bytes).map_err(|_| Error::InvalidKey)?;
        Ok(Self(bytes))
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            return Err(Error::KeyLength { expected: 32, got: bytes.len() });
        }
        let mut buf = [0u8; 32];
        buf.copy_from_slice(bytes);
        let out = Self::from_bytes(buf);
        buf.zeroize();
        out
    }

    pub fn from_hex(s: &str) -> Result<Self> {
        let mut bytes = hex::decode(s)?;
        let out = Self::from_slice(&bytes);
        bytes.zeroize();
        out
    }

    /// Fresh random key from getrandom (the device TRNG on Passport).
    pub fn generate() -> Result<Self> {
        loop {
            let mut buf = [0u8; 32];
            getrandom::getrandom(&mut buf).map_err(|_| Error::Rng)?;
            if let Ok(k) = Self::from_bytes(buf) {
                buf.zeroize();
                return Ok(k);
            }
        }
    }

    pub fn public_key(&self) -> PublicKey {
        let sk = self.signing_key();
        let vk: &VerifyingKey = sk.verifying_key();
        let mut out = [0u8; 32];
        out.copy_from_slice(&vk.to_bytes());
        PublicKey(out)
    }

    pub fn as_bytes(&self) -> &[u8; 32] { &self.0 }

    pub(crate) fn signing_key(&self) -> SigningKey { SigningKey::from_bytes(&self.0).expect("validated in ctor") }
}

impl fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretKey").field("pubkey", &self.public_key()).finish()
    }
}

impl PublicKey {
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self> {
        VerifyingKey::from_bytes(&bytes).map_err(|_| Error::InvalidKey)?;
        Ok(Self(bytes))
    }

    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            return Err(Error::KeyLength { expected: 32, got: bytes.len() });
        }
        let mut buf = [0u8; 32];
        buf.copy_from_slice(bytes);
        Self::from_bytes(buf)
    }

    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes = hex::decode(s)?;
        Self::from_slice(&bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] { &self.0 }

    pub fn to_hex(&self) -> String { hex::encode(self.0) }

    pub(crate) fn verifying_key(&self) -> VerifyingKey { VerifyingKey::from_bytes(&self.0).expect("validated in ctor") }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "PublicKey({})", self.to_hex()) }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.to_hex()) }
}

/// ECDH: x coordinate of sk * lift_x(peer), as NIP-44 specifies.
pub fn shared_secret(sk: &SecretKey, peer: &PublicKey) -> Result<[u8; 32]> {
    let peer_point = lift_x(peer.as_bytes())?;
    let scalar = sk_to_scalar(sk)?;
    let shared = ProjectivePoint::from(peer_point) * scalar;
    let affine = AffinePoint::from(shared);
    let encoded = affine.to_encoded_point(false);
    let bytes = encoded.as_bytes();
    if bytes.len() != 65 || bytes[0] != 0x04 {
        return Err(Error::Nip44("ecdh yielded unexpected point encoding"));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes[1..33]);
    Ok(out)
}

fn lift_x(x: &[u8; 32]) -> Result<AffinePoint> {
    let mut compressed = [0u8; 33];
    compressed[0] = 0x02;
    compressed[1..].copy_from_slice(x);
    let pk = K256Pub::from_sec1_bytes(&compressed).map_err(|_| Error::InvalidKey)?;
    Ok(*pk.as_affine())
}

fn sk_to_scalar(sk: &SecretKey) -> Result<Scalar> {
    use k256::elliptic_curve::generic_array::GenericArray;
    use k256::elliptic_curve::scalar::ScalarPrimitive;
    let ga = GenericArray::from_slice(sk.as_bytes());
    let sp: ScalarPrimitive<k256::Secp256k1> = ScalarPrimitive::from_bytes(ga).into_option().ok_or(Error::InvalidKey)?;
    Ok(sp.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nip06_reference_pubkey() {
        let sk = SecretKey::from_hex("7f7ff03d123792d6ac594bfa67bf6d0c0ab55b6b1fdb6249303fe861f1ccba9a").unwrap();
        assert_eq!(sk.public_key().to_hex(), "17162c921dc4d2518f9a101db33695df1afb56ab82f5ff3e5da6eec3ca5cd917");
    }

    #[test]
    fn ecdh_is_symmetric() {
        let a = SecretKey::generate().unwrap();
        let b = SecretKey::generate().unwrap();
        assert_eq!(shared_secret(&a, &b.public_key()).unwrap(), shared_secret(&b, &a.public_key()).unwrap());
    }

    #[test]
    fn rejects_bad_lengths_and_points() {
        assert!(matches!(SecretKey::from_slice(&[1u8; 31]), Err(Error::KeyLength { .. })));
        assert!(PublicKey::from_hex(&"f".repeat(64)).is_err());
        assert!(SecretKey::from_bytes([0u8; 32]).is_err());
    }
}
