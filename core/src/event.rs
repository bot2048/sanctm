// SPDX-FileCopyrightText: 2026 Foundation Devices, Inc. <hello@foundation.xyz>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// NIP-01 events. Signing/verification from the Passport Prime Nostr Signer
// (logic/nostr-core/src/event.rs, GPL-3.0-or-later; BIP-340 over the prehashed id,
// externally validated against nostr-tools). Rumors (NIP-59 unsigned events) added.

use k256::schnorr::signature::hazmat::{PrehashVerifier, RandomizedPrehashSigner};
use k256::schnorr::Signature;
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{Error, PublicKey, Result, SecretKey};

pub type Tags = Vec<Vec<String>>;

/// A signed event, as relays store it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Event {
    #[serde(with = "hex32")]
    pub id: [u8; 32],
    #[serde(with = "pubkey_hex")]
    pub pubkey: PublicKey,
    pub created_at: u64,
    pub kind: u32,
    pub tags: Tags,
    pub content: String,
    #[serde(with = "hex64")]
    pub sig: [u8; 64],
}

/// An event before signing. Also the shape of a NIP-59 rumor (id, no sig).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsignedEvent {
    pub pubkey: PublicKey,
    pub created_at: u64,
    pub kind: u32,
    pub tags: Tags,
    pub content: String,
}

/// NIP-59 rumor: an unsigned event carrying its id. Deniable: nobody can prove who wrote it
/// except through the signed seal around it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rumor {
    #[serde(with = "hex32")]
    pub id: [u8; 32],
    #[serde(with = "pubkey_hex")]
    pub pubkey: PublicKey,
    pub created_at: u64,
    pub kind: u32,
    pub tags: Tags,
    pub content: String,
}

pub fn event_id(pubkey: &PublicKey, created_at: u64, kind: u32, tags: &Tags, content: &str) -> Result<[u8; 32]> {
    // NIP-01: sha256 of [0, pubkey, created_at, kind, tags, content] with no whitespace.
    let value = serde_json::json!([0u8, pubkey.to_hex(), created_at, kind, tags, content]);
    let serialised = serde_json::to_string(&value)?;
    Ok(Sha256::digest(serialised.as_bytes()).into())
}

impl UnsignedEvent {
    pub fn id(&self) -> Result<[u8; 32]> { event_id(&self.pubkey, self.created_at, self.kind, &self.tags, &self.content) }

    /// BIP-340 Schnorr over the event id (prehash: the id is already a SHA-256), with
    /// auxiliary randomness from the TRNG.
    pub fn sign(self, sk: &SecretKey) -> Result<Event> {
        if sk.public_key() != self.pubkey {
            return Err(Error::InvalidKey);
        }
        let id = self.id()?;
        let sig: Signature = sk.signing_key().sign_prehash_with_rng(&mut OsRng, &id).map_err(|_| Error::InvalidKey)?;
        let mut sig_bytes = [0u8; 64];
        sig_bytes.copy_from_slice(&sig.to_bytes());
        Ok(Event {
            id,
            pubkey: self.pubkey,
            created_at: self.created_at,
            kind: self.kind,
            tags: self.tags,
            content: self.content,
            sig: sig_bytes,
        })
    }

    pub fn into_rumor(self) -> Result<Rumor> {
        let id = self.id()?;
        Ok(Rumor { id, pubkey: self.pubkey, created_at: self.created_at, kind: self.kind, tags: self.tags, content: self.content })
    }
}

impl Event {
    /// The id must hash the contents, and the signature must be a valid BIP-340 signature
    /// over the id by `pubkey`.
    pub fn verify(&self) -> Result<()> {
        let expected = event_id(&self.pubkey, self.created_at, self.kind, &self.tags, &self.content)?;
        if expected != self.id {
            return Err(Error::EventIdMismatch);
        }
        let sig = Signature::try_from(&self.sig[..]).map_err(|_| Error::BadSignature)?;
        self.pubkey.verifying_key().verify_prehash(&self.id, &sig).map_err(|_| Error::BadSignature)
    }

    pub fn id_hex(&self) -> String { hex::encode(self.id) }

    pub fn to_json(&self) -> Result<String> { Ok(serde_json::to_string(self)?) }

    pub fn from_json(s: &str) -> Result<Self> { Ok(serde_json::from_str(s)?) }

    /// Values of every tag named `name` (first element after the name).
    pub fn tag_values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a { tag_values(&self.tags, name) }
}

impl Rumor {
    pub fn check_id(&self) -> Result<()> {
        let expected = event_id(&self.pubkey, self.created_at, self.kind, &self.tags, &self.content)?;
        if expected != self.id {
            return Err(Error::EventIdMismatch);
        }
        Ok(())
    }

    pub fn id_hex(&self) -> String { hex::encode(self.id) }

    pub fn tag_values<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a { tag_values(&self.tags, name) }
}

pub fn tag_values<'a>(tags: &'a Tags, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    tags.iter().filter(move |t| t.first().map(String::as_str) == Some(name)).filter_map(|t| t.get(1).map(String::as_str))
}

// --- serde helpers for hex-encoded binary fields ----------------------------

mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 32], s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&hex::encode(bytes)) }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(&s).map_err(serde::de::Error::custom)?;
        v.try_into().map_err(|_| serde::de::Error::custom("expected 32-byte hex"))
    }
}

mod hex64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8; 64], s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&hex::encode(bytes)) }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let s = String::deserialize(d)?;
        let v = hex::decode(&s).map_err(serde::de::Error::custom)?;
        v.try_into().map_err(|_| serde::de::Error::custom("expected 64-byte hex"))
    }
}

pub(crate) mod pubkey_hex {
    use serde::{Deserialize, Deserializer, Serializer};

    use crate::PublicKey;

    pub fn serialize<S: Serializer>(pk: &PublicKey, s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&pk.to_hex()) }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<PublicKey, D::Error> {
        let s = String::deserialize(d)?;
        PublicKey::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sk() -> SecretKey { SecretKey::from_hex("7f7ff03d123792d6ac594bfa67bf6d0c0ab55b6b1fdb6249303fe861f1ccba9a").unwrap() }

    #[test]
    fn sign_then_verify_roundtrip() {
        let sk = sk();
        let ev = UnsignedEvent {
            pubkey: sk.public_key(),
            created_at: 1714078911,
            kind: 1,
            tags: vec![vec!["t".into(), "nostr".into()]],
            content: "gm \"quoted\" \\ \n ✓".into(),
        }
        .sign(&sk)
        .unwrap();
        ev.verify().unwrap();
        let back = Event::from_json(&ev.to_json().unwrap()).unwrap();
        back.verify().unwrap();
        assert_eq!(back, ev);
    }

    #[test]
    fn tampering_is_detected() {
        let sk = sk();
        let mut ev =
            UnsignedEvent { pubkey: sk.public_key(), created_at: 1, kind: 1, tags: vec![], content: "original".into() }
                .sign(&sk)
                .unwrap();
        ev.content = "forged".into();
        assert!(matches!(ev.verify(), Err(Error::EventIdMismatch)));
    }

    /// Signed by this code and confirmed with nostr-tools' verifyEvent (from the signer's suite).
    #[test]
    fn externally_validated_event_verifies() {
        let ev: Event = serde_json::from_value(serde_json::json!({
            "id": "6f1fe4853ffc63a8e3e6c634b7e740adbddbb08198221e3b998940264e2e0b3c",
            "pubkey": "634b0ca8cce792b32cf343162f106b8792570ffda74e27985e1515c4ec2f9c56",
            "created_at": 1_776_780_129u64,
            "kind": 1,
            "tags": [],
            "content": "Hello world!",
            "sig": "d06a16cd5b8ff82a690017d1391645ede662d47d2d696babb519c7670fcc1a06e683494eb377a4eaefd2069afa53d74f7f7a538e1154af073a27bca8147539da"
        }))
        .unwrap();
        ev.verify().unwrap();
    }

    #[test]
    fn rumor_roundtrip_keeps_id_and_has_no_sig() {
        let sk = sk();
        let rumor = UnsignedEvent { pubkey: sk.public_key(), created_at: 5, kind: 14, tags: vec![], content: "hi".into() }
            .into_rumor()
            .unwrap();
        let json = serde_json::to_string(&rumor).unwrap();
        assert!(!json.contains("sig"));
        let back: Rumor = serde_json::from_str(&json).unwrap();
        back.check_id().unwrap();
    }
}
