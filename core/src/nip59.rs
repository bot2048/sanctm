// SPDX-License-Identifier: GPL-3.0-or-later

//! NIP-59 gift wrap.
//!
//! ```text
//! rumor  (kind 14, unsigned, real author + real time)       <- what was said
//!   seal (kind 13, signed by the author, NIP-44 to receiver, random past time)
//!     wrap (kind 1059, signed by a one-time key, NIP-44 to receiver, random past time,
//!           ["p", receiver])                                  <- all a relay or the phone sees
//! ```
//!
//! A relay (and the companion phone carrying the wrap) learns only: some one-time key sent
//! something to `receiver` around some time in the last two days. Not who wrote it, not
//! when exactly, not what.

use serde::Deserialize;

use crate::event::{event_id, pubkey_hex, Tags};
use crate::{nip44, Error, Event, PublicKey, Result, Rumor, SecretKey, UnsignedEvent};

pub const KIND_SEAL: u32 = 13;
pub const KIND_GIFT_WRAP: u32 = 1059;
/// Seal and wrap timestamps are pushed back by up to two days (NIP-59), so relays can't
/// line up "sent" and "received" times. Subscriptions must look back this far.
pub const TIME_JITTER: u64 = 2 * 24 * 60 * 60;

/// `now` minus a uniformly random amount in `[0, TIME_JITTER)`.
pub fn tweaked_time(now: u64) -> Result<u64> {
    let mut b = [0u8; 4];
    getrandom::getrandom(&mut b).map_err(|_| Error::Rng)?;
    Ok(now.saturating_sub(u32::from_le_bytes(b) as u64 % TIME_JITTER))
}

/// Kind 13: the rumor, NIP-44-encrypted from `sender` to `receiver`, signed by `sender`.
pub fn seal(rumor: &Rumor, sender: &SecretKey, receiver: &PublicKey, now: u64) -> Result<Event> {
    if rumor.pubkey != sender.public_key() {
        return Err(Error::Wrap("rumor author is not the sender"));
    }
    let content = nip44::encrypt(sender, receiver, &serde_json::to_string(rumor)?)?;
    UnsignedEvent { pubkey: sender.public_key(), created_at: tweaked_time(now)?, kind: KIND_SEAL, tags: vec![], content }
        .sign(sender)
}

/// Kind 1059: the seal, NIP-44-encrypted from a fresh one-time key to `receiver`.
pub fn wrap_seal(seal: &Event, receiver: &PublicKey, now: u64) -> Result<Event> {
    let one_time = SecretKey::generate()?;
    let content = nip44::encrypt(&one_time, receiver, &seal.to_json()?)?;
    UnsignedEvent {
        pubkey: one_time.public_key(),
        created_at: tweaked_time(now)?,
        kind: KIND_GIFT_WRAP,
        tags: vec![vec!["p".into(), receiver.to_hex()]],
        content,
    }
    .sign(&one_time)
}

pub fn gift_wrap(rumor: &Rumor, sender: &SecretKey, receiver: &PublicKey, now: u64) -> Result<Event> {
    wrap_seal(&seal(rumor, sender, receiver, now)?, receiver, now)
}

pub struct Unwrapped {
    pub seal: Event,
    pub rumor: Rumor,
}

/// Open a gift wrap addressed to `me`. Checks the wrap and seal signatures, the rumor id,
/// and that the rumor's claimed author is the key that signed the seal.
pub fn unwrap(wrap: &Event, me: &SecretKey) -> Result<Unwrapped> {
    if wrap.kind != KIND_GIFT_WRAP {
        return Err(Error::Wrap("not a gift wrap"));
    }
    wrap.verify()?;
    let seal_json = nip44::decrypt(me, &wrap.pubkey, &wrap.content).map_err(|_| Error::Wrap("not addressed to this key"))?;
    let seal: Event = serde_json::from_str(&seal_json).map_err(|_| Error::Wrap("malformed seal"))?;
    if seal.kind != KIND_SEAL {
        return Err(Error::Wrap("inner event is not a seal"));
    }
    seal.verify()?;
    let rumor_json = nip44::decrypt(me, &seal.pubkey, &seal.content).map_err(|_| Error::Wrap("seal does not open"))?;
    let rumor = rumor_from_json_at(&rumor_json, seal.created_at)?;
    if rumor.pubkey != seal.pubkey {
        return Err(Error::Impersonation);
    }
    Ok(Unwrapped { seal, rumor })
}

/// Rumors from other clients: the id is recomputed (and must match if present); a stray
/// `sig` is ignored.
pub fn rumor_from_json(s: &str) -> Result<Rumor> { rumor_from_json_inner(s, None) }

/// As `rumor_from_json`, taking `fallback_time` (the seal's) when the rumor has no
/// `created_at`, as Amethyst does for clients that leave it out.
pub fn rumor_from_json_at(s: &str, fallback_time: u64) -> Result<Rumor> { rumor_from_json_inner(s, Some(fallback_time)) }

fn rumor_from_json_inner(s: &str, fallback_time: Option<u64>) -> Result<Rumor> {
    #[derive(Deserialize)]
    struct Wire {
        #[serde(default)]
        id: Option<String>,
        #[serde(with = "pubkey_hex")]
        pubkey: PublicKey,
        #[serde(default)]
        created_at: Option<u64>,
        kind: u32,
        tags: Tags,
        content: String,
    }
    let w: Wire = serde_json::from_str(s).map_err(|_| Error::Wrap("malformed rumor"))?;
    let created_at = w.created_at.or(fallback_time).ok_or(Error::Wrap("rumor without a time"))?;
    let id = event_id(&w.pubkey, created_at, w.kind, &w.tags, &w.content)?;
    if let Some(claimed) = w.id {
        if !claimed.eq_ignore_ascii_case(&hex::encode(id)) {
            return Err(Error::EventIdMismatch);
        }
    }
    Ok(Rumor { id, pubkey: w.pubkey, created_at, kind: w.kind, tags: w.tags, content: w.content })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rumor(sk: &SecretKey, to: &PublicKey, text: &str) -> Rumor {
        UnsignedEvent {
            pubkey: sk.public_key(),
            created_at: 1_790_000_000,
            kind: 14,
            tags: vec![vec!["p".into(), to.to_hex()]],
            content: text.into(),
        }
        .into_rumor()
        .unwrap()
    }

    #[test]
    fn wrap_and_unwrap() {
        let (alice, bob) = (SecretKey::generate().unwrap(), SecretKey::generate().unwrap());
        let r = rumor(&alice, &bob.public_key(), "hello bob");
        let w = gift_wrap(&r, &alice, &bob.public_key(), 1_790_000_000).unwrap();
        w.verify().unwrap();
        assert_eq!(w.kind, KIND_GIFT_WRAP);
        assert_ne!(w.pubkey, alice.public_key(), "wrap must not reveal the author");
        assert!(w.created_at <= 1_790_000_000 && w.created_at > 1_790_000_000 - TIME_JITTER);
        assert_eq!(w.tag_values("p").collect::<Vec<_>>(), vec![bob.public_key().to_hex()]);
        let u = unwrap(&w, &bob).unwrap();
        assert_eq!(u.rumor, r);
        assert_eq!(u.seal.pubkey, alice.public_key());
        // Alice can't open what she sent to Bob, and nobody else can either.
        assert!(unwrap(&w, &alice).is_err());
        assert!(unwrap(&w, &SecretKey::generate().unwrap()).is_err());
    }

    #[test]
    fn impersonation_is_refused() {
        // Mallory seals a rumor that claims Alice wrote it.
        let (alice, bob, mallory) =
            (SecretKey::generate().unwrap(), SecretKey::generate().unwrap(), SecretKey::generate().unwrap());
        let forged = rumor(&alice, &bob.public_key(), "send me your seed");
        let content = nip44::encrypt(&mallory, &bob.public_key(), &serde_json::to_string(&forged).unwrap()).unwrap();
        let seal = UnsignedEvent { pubkey: mallory.public_key(), created_at: 1, kind: KIND_SEAL, tags: vec![], content }
            .sign(&mallory)
            .unwrap();
        let w = wrap_seal(&seal, &bob.public_key(), 1_790_000_000).unwrap();
        assert!(matches!(unwrap(&w, &bob), Err(Error::Impersonation)));
        // And `seal` itself refuses to build one.
        assert!(super::seal(&forged, &mallory, &bob.public_key(), 1).is_err());
    }

    #[test]
    fn tampered_wrap_is_refused() {
        let (alice, bob) = (SecretKey::generate().unwrap(), SecretKey::generate().unwrap());
        let mut w = gift_wrap(&rumor(&alice, &bob.public_key(), "x"), &alice, &bob.public_key(), 1_790_000_000).unwrap();
        w.created_at += 1;
        assert!(unwrap(&w, &bob).is_err());
    }

    #[test]
    fn rumor_without_id_is_accepted_and_bad_id_is_not() {
        let alice = SecretKey::generate().unwrap();
        let r = rumor(&alice, &alice.public_key(), "note");
        let mut v = serde_json::to_value(&r).unwrap();
        v.as_object_mut().unwrap().remove("id");
        assert_eq!(rumor_from_json(&v.to_string()).unwrap(), r);
        v["id"] = serde_json::json!("00".repeat(32));
        assert!(rumor_from_json(&v.to_string()).is_err());
        // No created_at: the seal's time is used (and the id computed with it).
        let no_time = serde_json::json!({"pubkey": r.pubkey.to_hex(), "kind": 14, "tags": [], "content": "x"});
        assert!(rumor_from_json(&no_time.to_string()).is_err());
        let got = rumor_from_json_at(&no_time.to_string(), 1234).unwrap();
        assert_eq!(got.created_at, 1234);
        assert_eq!(got.id, crate::event::event_id(&r.pubkey, 1234, 14, &vec![], "x").unwrap());
    }
}
