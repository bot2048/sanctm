// SPDX-License-Identifier: GPL-3.0-or-later

//! Nostr profiles (kind 0) of the people we chat with, and our own.
//!
//! The companion fetches them from relays; the Passport checks the signature, so the phone
//! cannot put words in anyone's profile. Only display fields are kept, cleaned and clipped.
//! The profile picture arrives already shrunk to a small RGBA square (the phone decodes it;
//! the Passport only checks size), so a hostile image file never reaches a decoder here.

use std::collections::BTreeMap;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};

use crate::display::{clean_line, clean_text};
use crate::{Error, Event, PublicKey, Result};

pub const KIND_METADATA: u32 = 0;
/// Side of the square avatar the phone sends, in pixels.
pub const AVATAR_PX: u32 = 64;
pub const MAX_PROFILES: usize = 600;
const MAX_NAME: usize = 40;
const MAX_ABOUT: usize = 280;
const MAX_URL: usize = 512;
const MAX_NIP05: usize = 100;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// `display_name`, else `name`. Empty when the profile sets neither.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// `name` when it differs from the display name (a handle).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub handle: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub about: String,
    /// `https://` image URL.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub picture: String,
    /// NIP-05 address as published (`alice@example.com`, `_@example.com`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub nip05: String,
    /// The phone looked the NIP-05 address up and it names this key.
    #[serde(default)]
    pub nip05_ok: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lud16: String,
    pub created_at: u64,
    /// AVATAR_PX x AVATAR_PX RGBA of `avatar_url`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<Avatar>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Avatar {
    pub url: String,
    #[serde(with = "b64")]
    pub rgba: Vec<u8>,
}

mod b64 {
    use super::B64;
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> { s.serialize_str(&B64.encode(v)) }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        B64.decode(s).map_err(serde::de::Error::custom)
    }
}

impl Profile {
    /// "Alice", else "alice@example.com", else None.
    pub fn label(&self) -> Option<String> {
        if !self.name.is_empty() {
            Some(self.name.clone())
        } else if !self.nip05.is_empty() {
            Some(nip05_display(&self.nip05))
        } else {
            None
        }
    }
}

/// `_@example.com` is shown as `example.com` (NIP-05).
pub fn nip05_display(id: &str) -> String { id.strip_prefix("_@").map(str::to_string).unwrap_or_else(|| id.to_string()) }

/// Characters of a NIP-05 address shown in one line (rows, headers): longer ones lose their
/// start, never their end.
pub const NIP05_SHOWN: usize = 28;

/// `nip05_display`, at most `max` characters. The UI elides at the end, which would hide the
/// part that vouches for the name: "jack@primal.net.verified-users.example" became
/// "jack@primal.net.verif…" with a check mark. Cut at the start instead, so the end of the domain
/// always shows: "…et.verified-users.example".
pub fn nip05_short(id: &str, max: usize) -> String {
    let d = nip05_display(id);
    let n = d.chars().count();
    if n <= max || max < 2 {
        return d;
    }
    let tail: String = d.chars().skip(n - (max - 1)).collect();
    format!("…{tail}")
}

static LOCAL_HTTP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Allow `http://localhost` / `http://127.0.0.1` picture links (the simulator's local test
/// server). Off by default: on a real device a stranger's link must not make the phone call
/// services on its own machine.
pub fn allow_local_http(on: bool) { LOCAL_HTTP.store(on, std::sync::atomic::Ordering::Relaxed) }

fn local_http(s: &str) -> bool {
    if !LOCAL_HTTP.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let Some(rest) = s.strip_prefix("http://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once(':').filter(|(_, p)| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())).map(|(h, _)| h).unwrap_or(host);
    host == "localhost" || host == "127.0.0.1"
}

/// `https://` URLs only (plus local `http://` when `allow_local_http` is on).
pub fn https_url(s: &str) -> String {
    let s = s.trim();
    let local = local_http(s);
    let ok = s.len() <= MAX_URL
        && (s.starts_with("https://") || local)
        && s.len() > 8
        && s.chars().all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '<' | '>' | '\\' | '^' | '`' | '{' | '|' | '}'));
    if ok {
        s.to_string()
    } else {
        String::new()
    }
}

/// `name@domain` (lowercase; letters, digits, `-_.` in the name; a dotted domain), or None.
pub fn parse_nip05(s: &str) -> Option<String> {
    let s = s.trim().trim_start_matches('@').to_ascii_lowercase();
    let (name, domain) = match s.split_once('@') {
        Some((n, d)) => (n.to_string(), d.to_string()),
        None => ("_".to_string(), s.clone()),
    };
    let name_ok = !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    let label_ok = |l: &str| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !l.starts_with('-') && !l.ends_with('-');
    let domain_ok = domain.len() <= 253 && domain.contains('.') && domain.split('.').all(label_ok);
    if name_ok && domain_ok && name.len() + 1 + domain.len() <= MAX_NIP05 {
        Some(format!("{name}@{domain}"))
    } else {
        None
    }
}

/// Check and read a kind 0 event.
pub fn parse(ev: &Event) -> Result<Profile> {
    if ev.kind != KIND_METADATA {
        return Err(Error::Frame("not a profile"));
    }
    ev.verify()?;
    if ev.content.len() > 64 * 1024 {
        return Err(Error::Frame("profile too large"));
    }
    // A profile is one flat object; deep nesting would only cost stack.
    if crate::link::depth_exceeds(&ev.content, 4) {
        return Err(Error::Frame("profile nested too deeply"));
    }
    let v: serde_json::Value = serde_json::from_str(&ev.content).map_err(|_| Error::Frame("profile is not JSON"))?;
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    let display = clean_line(field("display_name"), MAX_NAME);
    let display = if display.is_empty() { clean_line(field("displayName"), MAX_NAME) } else { display };
    let handle = clean_line(field("name"), MAX_NAME);
    let name = if display.is_empty() { handle.clone() } else { display };
    let handle = if handle.eq_ignore_ascii_case(&name) { String::new() } else { handle };
    let about: String = clean_text(field("about")).chars().take(MAX_ABOUT).collect();
    let lud16 = field("lud16").trim();
    Ok(Profile {
        name,
        handle,
        about: about.trim().to_string(),
        picture: https_url(field("picture")),
        nip05: parse_nip05(field("nip05")).filter(|_| field("nip05").contains('@')).unwrap_or_default(),
        nip05_ok: false,
        lud16: if lud16.len() <= MAX_NIP05 && parse_nip05(lud16).is_some() && lud16.contains('@') { lud16.to_ascii_lowercase() } else { String::new() },
        created_at: ev.created_at,
        avatar: None,
    })
}

/// Profiles we keep, by hex key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profiles {
    pub by_key: BTreeMap<String, Profile>,
}

impl Profiles {
    pub fn get(&self, pk: &PublicKey) -> Option<&Profile> { self.by_key.get(&pk.to_hex()) }

    /// Store a newer profile. Keeps the avatar while the picture URL is unchanged. Returns
    /// whether anything changed.
    pub fn put(&mut self, pk: &PublicKey, mut p: Profile, nip05_ok: bool) -> bool {
        let key = pk.to_hex();
        p.nip05_ok = nip05_ok && !p.nip05.is_empty();
        match self.by_key.get(&key) {
            Some(old) if old.created_at > p.created_at => return false,
            Some(old) if old.created_at == p.created_at && old.nip05_ok == p.nip05_ok => return false,
            Some(old) => {
                if old.avatar.as_ref().is_some_and(|a| a.url == p.picture) {
                    p.avatar = old.avatar.clone();
                }
            }
            None => {}
        }
        if !self.by_key.contains_key(&key) && self.by_key.len() >= MAX_PROFILES {
            return false;
        }
        self.by_key.insert(key, p);
        true
    }

    /// An avatar for `pk`, if it is the picture its profile currently names.
    pub fn set_avatar(&mut self, pk: &PublicKey, url: &str, rgba: Vec<u8>) -> bool {
        if rgba.len() != (AVATAR_PX * AVATAR_PX * 4) as usize {
            return false;
        }
        match self.by_key.get_mut(&pk.to_hex()) {
            Some(p) if p.picture == url && !url.is_empty() => {
                p.avatar = Some(Avatar { url: url.to_string(), rgba });
                true
            }
            _ => false,
        }
    }

    /// Profiles whose picture has not been fetched yet.
    pub fn missing_avatars(&self) -> Vec<(String, String)> {
        self.by_key
            .iter()
            .filter(|(_, p)| !p.picture.is_empty() && p.avatar.as_ref().map(|a| a.url != p.picture).unwrap_or(true))
            .map(|(k, p)| (k.clone(), p.picture.clone()))
            .collect()
    }

    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) { self.by_key.retain(|k, _| keep(k)); }
}

pub fn decode_rgba(b64: &str) -> Result<Vec<u8>> { Ok(B64.decode(b64)?) }

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SecretKey, UnsignedEvent};

    fn kind0(sk: &SecretKey, content: &str, at: u64) -> Event {
        UnsignedEvent { pubkey: sk.public_key(), created_at: at, kind: 0, tags: vec![], content: content.into() }.sign(sk).unwrap()
    }

    #[test]
    fn reads_and_cleans_profiles() {
        let sk = SecretKey::generate().unwrap();
        let content = serde_json::json!({
            "name": "alice",
            "display_name": "Alice \u{202E}B",
            "about": "hi\u{2028}there",
            "picture": "https://x.example/a.png",
            "nip05": "Alice@Example.com",
            "lud16": "alice@getalby.com"
        });
        let ev = kind0(&sk, &content.to_string(), 5);
        let p = parse(&ev).unwrap();
        assert_eq!(p.name, "Alice B");
        assert_eq!(p.handle, "alice");
        assert_eq!(p.about, "hi\nthere");
        assert_eq!(p.picture, "https://x.example/a.png");
        assert_eq!(p.nip05, "alice@example.com");
        assert_eq!(p.lud16, "alice@getalby.com");
        assert_eq!(p.label().unwrap(), "Alice B");
        // Names only, or only a NIP-05.
        let p = parse(&kind0(&sk, r#"{"name":"bob"}"#, 1)).unwrap();
        assert_eq!((p.name.as_str(), p.handle.as_str()), ("bob", ""));
        let p = parse(&kind0(&sk, r#"{"nip05":"_@bob.dev"}"#, 1)).unwrap();
        assert_eq!(p.label().unwrap(), "bob.dev");
    }

    #[test]
    fn refuses_bad_profiles() {
        let sk = SecretKey::generate().unwrap();
        let mut ev = kind0(&sk, r#"{"name":"mallory"}"#, 1);
        ev.content = r#"{"name":"alice"}"#.into();
        assert!(parse(&ev).is_err(), "forged content");
        assert!(parse(&kind0(&sk, "not json", 1)).is_err());
        let p = parse(&kind0(&sk, r#"{"picture":"http://insecure/a.png","nip05":"not an address"}"#, 1)).unwrap();
        assert!(p.picture.is_empty() && p.nip05.is_empty());
        let p = parse(&kind0(&sk, r#"{"picture":"javascript:alert(1)"}"#, 1)).unwrap();
        assert!(p.picture.is_empty());
    }

    #[test]
    fn long_nip05_keeps_its_domain_end() {
        assert_eq!(nip05_short("alice@damus.io", NIP05_SHOWN), "alice@damus.io");
        assert_eq!(nip05_short("_@jack.dev", NIP05_SHOWN), "jack.dev");
        let spoof = parse_nip05("jack@primal.net.verified-users.example").unwrap();
        let shown = nip05_short(&spoof, NIP05_SHOWN);
        assert_eq!(shown.chars().count(), NIP05_SHOWN);
        assert!(shown.ends_with("verified-users.example") && !shown.contains("jack@primal.net"), "{shown}");
        let long_name = parse_nip05(&format!("{}@x.io", "a".repeat(60))).unwrap();
        assert!(nip05_short(&long_name, NIP05_SHOWN).ends_with("@x.io"));
        // Unicode never reaches here: look-alike, bidi and zero-width addresses are refused.
        for bad in ["jack@pr\u{0456}mal.net", "jack@primal.net\u{202e}", "ja\u{200b}ck@primal.net", "jack@primal.net/x"] {
            assert!(parse_nip05(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn nip05_grammar() {
        assert_eq!(parse_nip05("@Alice@Example.COM").unwrap(), "alice@example.com");
        assert_eq!(parse_nip05("example.com").unwrap(), "_@example.com");
        assert!(parse_nip05("alice@localhost").is_none());
        assert!(parse_nip05("al ice@x.com").is_none());
        assert!(parse_nip05("alice@x.com/../y").is_none());
        assert!(parse_nip05("alice@ex\u{0430}mple.com").is_none());
        assert!(parse_nip05("npub1xyz").is_none());
    }

    #[test]
    fn newest_wins_and_avatars_follow_the_picture() {
        let sk = SecretKey::generate().unwrap();
        let pk = sk.public_key();
        let mut ps = Profiles::default();
        assert!(ps.put(&pk, parse(&kind0(&sk, r#"{"name":"a","picture":"https://x/1.png"}"#, 10)).unwrap(), false));
        assert!(!ps.put(&pk, parse(&kind0(&sk, r#"{"name":"old"}"#, 9)).unwrap(), false), "older ignored");
        assert_eq!(ps.missing_avatars(), vec![(pk.to_hex(), "https://x/1.png".to_string())]);
        assert!(!ps.set_avatar(&pk, "https://x/1.png", vec![0; 10]), "wrong size");
        assert!(!ps.set_avatar(&pk, "https://x/other.png", vec![0; (AVATAR_PX * AVATAR_PX * 4) as usize]), "not its picture");
        assert!(ps.set_avatar(&pk, "https://x/1.png", vec![7; (AVATAR_PX * AVATAR_PX * 4) as usize]));
        assert!(ps.missing_avatars().is_empty());
        // A newer profile with the same picture keeps the avatar; a new picture drops it.
        assert!(ps.put(&pk, parse(&kind0(&sk, r#"{"name":"b","picture":"https://x/1.png"}"#, 11)).unwrap(), true));
        assert!(ps.get(&pk).unwrap().avatar.is_some());
        assert!(ps.put(&pk, parse(&kind0(&sk, r#"{"name":"c","picture":"https://x/2.png"}"#, 12)).unwrap(), false));
        assert!(ps.get(&pk).unwrap().avatar.is_none());
        let json = serde_json::to_string(&ps).unwrap();
        assert_eq!(serde_json::from_str::<Profiles>(&json).unwrap(), ps);
    }
}
