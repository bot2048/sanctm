// SPDX-License-Identifier: GPL-3.0-or-later

//! The Passport <-> companion link protocol: one JSON object per frame.
//!
//! The transport is whatever carries bytes between the Passport and the phone: today the
//! simulator's WebSocket stand-in, on hardware QuantumLink once KeyOS opens an app data
//! channel (QLv2, KeyOS 1.5). The protocol doesn't change with the transport.
//!
//! Only public data crosses it: the Passport's npub, signed gift wraps (ciphertext with a
//! one-time author), a signed relay list and relay AUTH events. Never plaintext, never keys,
//! never contact names.
//!
//! ```text
//! companion -> Passport
//!   {"t":"hello","v":1,"client":"…","now":1790000000,"relays":["wss://…"]}
//!   {"t":"event","ev":{kind 1059 …},"relay":"wss://…"}      a gift wrap p-tagged to us
//!   {"t":"eose"}                                            initial sync finished
//!   {"t":"ok","id":"<wrap id>","accepted":["wss://…"],"failed":["wss://…: reason"]}
//!   {"t":"auth","relay":"wss://…","challenge":"…"}          NIP-42, sign it please
//!   {"t":"relays","up":3,"total":4}
//!   {"t":"ping"} | {"t":"pong"}
//!   {"t":"profile","ev":{kind 0},"nip05_ok":true}          a profile the Passport asked for
//!   {"t":"nip05","q":"alice@example.com","pubkey":"<hex>"}  a NIP-05 lookup answered
//!   {"t":"blob","id":"…","seq":0,"last":true,"w":64,"h":64,"data":"<base64>"}
//! Passport -> companion
//!   {"t":"hello","v":1,"app":"nip-ping","pubkey":"<hex>","since":1789000000}
//!   {"t":"publish","ev":{…},"to":"<hex>"|null}               to = whose inbox relays to use
//!   {"t":"auth","ev":{kind 22242 …}}
//!   {"t":"profiles","pks":["<hex>",…]}                     fetch these kind 0s
//!   {"t":"nip05","q":"alice@example.com"}                   resolve an address
//!   {"t":"fetch","id":"…","url":"https://…","as":"avatar"|"picture"|"file","w":…,"h":…,"max":…}
//!   {"t":"ping"} | {"t":"pong"}
//! ```
//!
//! Profiles, avatars and pictures tell the phone whose profile or which link you look at;
//! encrypted files stay encrypted until they are on the Passport.

use serde::Deserialize;
use serde_json::json;

use crate::{Error, Event, PublicKey, Result};

pub const VERSION: u32 = 1;
pub const APP: &str = "nip-ping";
/// Biggest frame the Passport will parse. A gift wrap with a 2000-byte message is ~6 KB.
pub const MAX_FRAME_BYTES: usize = 128 * 1024;

#[derive(Debug, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Inbound {
    Hello {
        #[serde(default)]
        v: u32,
        #[serde(default)]
        client: String,
        #[serde(default)]
        now: Option<u64>,
        #[serde(default)]
        relays: Vec<String>,
    },
    Event {
        ev: serde_json::Value,
        #[serde(default)]
        relay: String,
    },
    Eose {},
    Ok {
        id: String,
        #[serde(default)]
        accepted: Vec<String>,
        #[serde(default)]
        failed: Vec<String>,
    },
    Auth {
        relay: String,
        challenge: String,
    },
    Relays {
        #[serde(default)]
        up: u32,
        #[serde(default)]
        total: u32,
    },
    Ping {},
    Pong {},
    /// A kind 0 the Passport asked for; `nip05_ok`: the phone resolved its NIP-05 to this key.
    Profile {
        ev: serde_json::Value,
        #[serde(default)]
        nip05_ok: bool,
    },
    /// Answer to a NIP-05 lookup.
    Nip05 {
        q: String,
        #[serde(default)]
        pubkey: Option<String>,
        #[serde(default)]
        error: Option<String>,
    },
    /// Part of a fetched file: base64 `data`, in order; `last` on the final part. For an avatar
    /// or a picture the phone sends RGBA pixels of size `w` x `h`; for an encrypted file, the
    /// ciphertext as stored on the server.
    Blob {
        id: String,
        #[serde(default)]
        seq: u32,
        #[serde(default)]
        last: bool,
        #[serde(default)]
        w: u32,
        #[serde(default)]
        h: u32,
        #[serde(default)]
        data: String,
        #[serde(default)]
        error: Option<String>,
    },
}

/// Deepest JSON nesting a frame may have. A real frame is 4 levels deep (frame, event, tags,
/// tag); serde_json allows 128, and each level costs stack on a 256 KiB process.
pub const MAX_DEPTH: usize = 16;

/// Nesting depth of a JSON text, counting brackets outside strings. Stops early past `limit`.
pub(crate) fn depth_exceeds(text: &str, limit: usize) -> bool {
    let (mut depth, mut in_str, mut esc) = (0usize, false, false);
    for b in text.bytes() {
        if in_str {
            match b {
                _ if esc => esc = false,
                b'\\' => esc = true,
                b'"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'[' | b'{' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    false
}

pub fn parse(text: &str) -> Result<Inbound> {
    if text.len() > MAX_FRAME_BYTES {
        return Err(Error::Frame("frame too large"));
    }
    if depth_exceeds(text, MAX_DEPTH) {
        return Err(Error::Frame("frame nested too deeply"));
    }
    serde_json::from_str(text).map_err(|_| Error::Frame("not a link frame"))
}

pub fn hello(me: &PublicKey, since: u64) -> String {
    json!({"t": "hello", "v": VERSION, "app": APP, "pubkey": me.to_hex(), "since": since}).to_string()
}

pub fn publish(ev: &Event, to: Option<&PublicKey>) -> String {
    json!({"t": "publish", "ev": ev, "to": to.map(|p| p.to_hex())}).to_string()
}

pub fn auth(ev: &Event) -> String { json!({"t": "auth", "ev": ev}).to_string() }

pub fn ping() -> String { json!({"t": "ping"}).to_string() }

/// Ask for these keys' kind 0 profiles.
pub fn want_profiles(pks: &[String]) -> String { json!({"t": "profiles", "pks": pks}).to_string() }

/// Resolve a NIP-05 address (`name@domain`) to a key.
pub fn nip05(q: &str) -> String { json!({"t": "nip05", "q": q}).to_string() }

/// Fetch `url`. `as_`: "avatar" (RGBA square), "picture" (RGBA, fit in w x h) or "file"
/// (raw bytes, at most `max` of them).
pub fn fetch(id: &str, url: &str, as_: &str, w: u32, h: u32, max: usize) -> String {
    json!({"t": "fetch", "id": id, "url": url, "as": as_, "w": w, "h": h, "max": max}).to_string()
}

pub fn pong() -> String { json!({"t": "pong"}).to_string() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_inbound_frame() {
        assert!(matches!(parse(r#"{"t":"hello","v":1,"now":5,"relays":["wss://a"]}"#).unwrap(), Inbound::Hello { now: Some(5), .. }));
        assert!(matches!(parse(r#"{"t":"hello"}"#).unwrap(), Inbound::Hello { v: 0, now: None, .. }));
        assert!(matches!(parse(r#"{"t":"event","ev":{"kind":1059}}"#).unwrap(), Inbound::Event { .. }));
        assert!(matches!(parse(r#"{"t":"eose"}"#).unwrap(), Inbound::Eose {}));
        assert!(matches!(parse(r#"{"t":"ok","id":"ab","accepted":["wss://a"]}"#).unwrap(), Inbound::Ok { .. }));
        assert!(matches!(parse(r#"{"t":"auth","relay":"wss://a","challenge":"x"}"#).unwrap(), Inbound::Auth { .. }));
        assert!(matches!(parse(r#"{"t":"relays","up":2,"total":4}"#).unwrap(), Inbound::Relays { up: 2, total: 4 }));
        assert!(matches!(parse(r#"{"t":"ping"}"#).unwrap(), Inbound::Ping {}));
        assert!(parse(r#"{"t":"nope"}"#).is_err());
        assert!(parse("not json").is_err());
        assert!(parse(&format!(r#"{{"t":"ping","pad":"{}"}}"#, "x".repeat(MAX_FRAME_BYTES))).is_err());
        let deep = |n: usize| format!(r#"{{"t":"ping","x":{}{}}}"#, "[".repeat(n), "]".repeat(n));
        assert!(parse(&deep(MAX_DEPTH - 1)).is_ok());
        assert!(parse(&deep(MAX_DEPTH)).is_err(), "too deep");
        // Brackets inside strings (escaped quotes too) do not count.
        let s = format!(r#"{{"t":"ping","x":"\"{}"}}"#, "[".repeat(100));
        assert!(parse(&s).is_ok());
    }

    #[test]
    fn outbound_frames_are_json() {
        let pk = crate::SecretKey::generate().unwrap().public_key();
        let h: serde_json::Value = serde_json::from_str(&hello(&pk, 42)).unwrap();
        assert_eq!(h["t"], "hello");
        assert_eq!(h["pubkey"], pk.to_hex());
        assert_eq!(h["since"], 42);
    }
}
