// SPDX-License-Identifier: GPL-3.0-or-later

//! NIP-17 private direct messages (kind 14 chat rumors inside NIP-59 gift wraps), plus the
//! two small signed events the app needs around them: the kind 10050 DM relay list and the
//! kind 22242 NIP-42 relay AUTH.
//!
//! Every message is wrapped twice: once for the recipient and once for the sender's own
//! key, so the sender's other devices (or this Passport after a restore) see the thread.

use crate::nip59::{self, KIND_GIFT_WRAP};
use crate::{Error, Event, PublicKey, Result, Rumor, SecretKey, UnsignedEvent};

pub const KIND_CHAT: u32 = 14;
pub const KIND_FILE: u32 = 15;
pub const KIND_DM_RELAYS: u32 = 10050;
pub const KIND_AUTH: u32 = 22242;
/// Longest message the Passport will compose (bytes of UTF-8).
pub const MAX_TEXT_BYTES: usize = 2000;
/// Longest message the Passport will keep and display; longer ones are cut with a note.
/// (Together with the inbox byte budget this bounds what strangers can make us store.)
pub const MAX_SHOWN_BYTES: usize = 4000;
/// Longest subject shown (characters).
pub const MAX_SUBJECT_CHARS: usize = 100;
/// A rumor naming more people than this is a broadcast, not a chat: only the first are kept.
pub const MAX_RECIPIENTS: usize = 64;
/// A gift wrap carries one `p` tag (and maybe an expiration); more is not a NIP-59 wrap.
pub const MAX_WRAP_TAGS: usize = 8;

pub fn check_text(text: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Err(Error::Empty);
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(Error::TooLong { got: text.len(), max: MAX_TEXT_BYTES });
    }
    Ok(())
}

pub fn chat_rumor(sender: &PublicKey, to: &[PublicKey], text: &str, now: u64) -> Result<Rumor> {
    check_text(text)?;
    UnsignedEvent {
        pubkey: *sender,
        created_at: now,
        kind: KIND_CHAT,
        tags: to.iter().map(|p| vec!["p".to_string(), p.to_hex()]).collect(),
        content: text.to_string(),
    }
    .into_rumor()
}

/// A reply: a kind 14 with `["e", <parent rumor id>, "", "reply"]` (NIP-17).
pub fn reply_rumor(sender: &PublicKey, to: &PublicKey, text: &str, parent: &str, now: u64) -> Result<Rumor> {
    check_text(text)?;
    if !is_hex_id(parent) {
        return Err(Error::Frame("bad reply target"));
    }
    UnsignedEvent {
        pubkey: *sender,
        created_at: now,
        kind: KIND_CHAT,
        tags: vec![vec!["p".into(), to.to_hex()], vec!["e".into(), parent.to_ascii_lowercase(), String::new(), "reply".into()]],
        content: text.to_string(),
    }
    .into_rumor()
}

/// A reaction (kind 7, NIP-25) to message `target` written by `author`, inside the chat with `to`.
pub fn reaction_rumor(sender: &PublicKey, to: &PublicKey, target: &str, target_kind: u32, author: &PublicKey, emoji: &str, now: u64) -> Result<Rumor> {
    let emoji = emoji.trim();
    if !is_hex_id(target) || emoji.is_empty() || emoji.chars().count() > 8 {
        return Err(Error::Frame("bad reaction"));
    }
    let mut tags = vec![vec!["e".into(), target.to_ascii_lowercase()], vec!["p".into(), to.to_hex()], vec!["k".into(), target_kind.to_string()]];
    if author != to && author != sender {
        tags.push(vec!["p".into(), author.to_hex()]);
    }
    UnsignedEvent { pubkey: *sender, created_at: now, kind: KIND_REACTION, tags, content: emoji.to_string() }.into_rumor()
}

/// A delete (kind 5, NIP-09) of our own earlier rumors `targets` of kind `kind`, inside the
/// chat with `to` (e.g. taking back a reaction when it is changed).
pub fn delete_rumor(sender: &PublicKey, to: &PublicKey, targets: &[String], kind: u32, now: u64) -> Result<Rumor> {
    if targets.is_empty() || !targets.iter().all(|t| is_hex_id(t)) {
        return Err(Error::Frame("bad delete"));
    }
    let mut tags: Vec<Vec<String>> = targets.iter().map(|t| vec!["e".into(), t.to_ascii_lowercase()]).collect();
    tags.push(vec!["k".into(), kind.to_string()]);
    tags.push(vec!["p".into(), to.to_hex()]);
    UnsignedEvent { pubkey: *sender, created_at: now, kind: KIND_DELETE, tags, content: String::new() }.into_rumor()
}

/// Gift-wrap any rumor for `to` and for ourselves (once, if `to` is us).
pub fn wrap_for(me: &SecretKey, to: &PublicKey, rumor: Rumor, now: u64) -> Result<Outgoing> {
    let my_pk = me.public_key();
    let mut wraps = vec![(*to, nip59::gift_wrap(&rumor, me, to, now)?)];
    if *to != my_pk {
        wraps.push((my_pk, nip59::gift_wrap(&rumor, me, &my_pk, now)?));
    }
    Ok(Outgoing { rumor, wraps })
}

/// One message, ready for relays: a wrap per recipient plus the copy for ourselves.
pub struct Outgoing {
    pub rumor: Rumor,
    /// (recipient, wrap) pairs; the last one is always the self copy.
    pub wraps: Vec<(PublicKey, Event)>,
}

pub fn send_dm(me: &SecretKey, to: &PublicKey, text: &str, now: u64) -> Result<Outgoing> {
    let rumor = chat_rumor(&me.public_key(), &[*to], text, now)?;
    wrap_for(me, to, rumor, now)
}

pub const KIND_REACTION: u32 = 7;
pub const KIND_DELETE: u32 = 5;

/// Kind 15 file message (NIP-17): an encrypted file on a server, with what opens it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FileMeta {
    pub url: String,
    /// `file-type` as sent (may be wrong or empty: 0xchat sends image/png for every image).
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub algo: String,
    pub key: String,
    pub nonce: String,
    /// SHA-256 of the encrypted file (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<String>,
}

impl FileMeta {
    /// "Photo", "Video", "Voice message", "File".
    pub fn label(&self) -> &'static str {
        let m = self.mime.to_ascii_lowercase();
        let u = self.url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        let ext = |e: &[&str]| e.iter().any(|x| u.ends_with(x));
        if m.starts_with("video/") || ext(&[".mp4", ".mov", ".webm", ".m4v"]) {
            "Video"
        } else if m.starts_with("audio/") || ext(&[".mp3", ".m4a", ".ogg", ".wav", ".aac", ".opus"]) {
            "Voice message"
        } else if m.starts_with("image/") || ext(&[".jpg", ".jpeg", ".png", ".gif", ".webp"]) {
            "Photo"
        } else {
            "File"
        }
    }

    pub fn is_image(&self) -> bool { self.label() == "Photo" }
}

/// What a wrapped rumor carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// Kind 14 text, or a kind 15 file (text = its caption, maybe empty).
    Message { text: String, file: Option<FileMeta>, reply_to: Option<String> },
    /// Kind 7: `emoji` on the message `target` (rumor id).
    Reaction { target: String, emoji: String },
    /// Kind 5: the author takes back these messages (rumor ids).
    Delete { targets: Vec<String> },
}

/// A message opened on this Passport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dm {
    /// Rumor id (hex): the same for every wrapped copy of one message.
    pub id: String,
    pub sender: PublicKey,
    /// `p` tags of the rumor: everyone in the chat except the sender.
    pub recipients: Vec<PublicKey>,
    pub created_at: u64,
    pub subject: Option<String>,
    pub kind: u32,
    /// NIP-40: gone after this time.
    pub expires: Option<u64>,
    pub body: Body,
}

impl Dm {
    /// Text of a message (empty for reactions and deletes), for tests and tools.
    pub fn text(&self) -> &str {
        match &self.body {
            Body::Message { text, .. } => text,
            _ => "",
        }
    }
}

fn is_hex_id(s: &str) -> bool { s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()) }

/// The message a rumor answers: an `e` tag marked "reply", else one marked "root", else the
/// last unmarked `e` tag (NIP-10 positional: root first, parent last), else a `q` tag
/// (0xchat, Flotilla) holding a plain id. "mention" `e` tags are not replies.
fn reply_target(rumor: &crate::Rumor) -> Option<String> {
    let es: Vec<&Vec<String>> = rumor.tags.iter().filter(|t| t.first().map(String::as_str) == Some("e") && t.get(1).is_some_and(|v| is_hex_id(v))).collect();
    let marker = |t: &&&Vec<String>, m: &str| t.get(3).map(String::as_str).unwrap_or("") == m;
    let marked = es.iter().find(|t| marker(t, "reply")).or_else(|| es.iter().find(|t| marker(t, "root")));
    marked
        .or_else(|| es.iter().rev().find(|t| marker(t, "")))
        .map(|t| t[1].to_ascii_lowercase())
        .or_else(|| rumor.tag_values("q").find(|v| is_hex_id(v)).map(str::to_ascii_lowercase))
}

fn file_meta(rumor: &crate::Rumor) -> FileMeta {
    let tag = |k: &str| rumor.tag_values(k).next().map(|v| v.trim().to_string());
    let small = |v: Option<String>, n: usize| v.filter(|v| v.len() <= n && v.chars().all(|c| c.is_ascii_graphic()));
    FileMeta {
        url: small(Some(rumor.content.trim().to_string()), 1000).unwrap_or_default(),
        mime: small(tag("file-type"), 100).unwrap_or_default(),
        algo: small(tag("encryption-algorithm"), 40).unwrap_or_default(),
        key: small(tag("decryption-key"), 200).unwrap_or_default(),
        nonce: small(tag("decryption-nonce"), 200).unwrap_or_default(),
        x: small(tag("x"), 64).filter(|v| is_hex_id(v)),
        size: tag("size").and_then(|v| v.parse().ok()),
        dim: small(tag("dim"), 20),
    }
}

pub fn open_dm(wrap: &Event, me: &SecretKey) -> Result<Dm> {
    let nip59::Unwrapped { rumor, seal } = nip59::unwrap(wrap, me)?;
    let body = match rumor.kind {
        KIND_CHAT => Body::Message { text: clip(&rumor.content), file: None, reply_to: reply_target(&rumor) },
        KIND_FILE => {
            let caption = rumor.tag_values("alt").next().map(clip).unwrap_or_default();
            Body::Message { text: caption, file: Some(file_meta(&rumor)), reply_to: reply_target(&rumor) }
        }
        KIND_REACTION => {
            // NIP-25: the last `e` tag is the reacted-to event.
            let target = rumor
                .tags
                .iter()
                .filter(|t| t.first().map(String::as_str) == Some("e") && t.get(1).is_some_and(|v| is_hex_id(v)))
                .last()
                .map(|t| t[1].to_ascii_lowercase())
                .ok_or(Error::Wrap("reaction to nothing"))?;
            let emoji = match rumor.content.trim() {
                "" | "+" => "❤️".to_string(),
                "-" => "👎".to_string(),
                // Custom emoji images (":name:") and anything wordy ("Verified") become a star:
                // a reaction is a symbol, never text someone can make the Passport say.
                e if e.starts_with(':') || e.chars().any(|c| c.is_alphanumeric()) || e.chars().count() > 8 => "★".to_string(),
                e => crate::display::clean_text(e).replace('\n', " "),
            };
            Body::Reaction { target, emoji }
        }
        KIND_DELETE => Body::Delete { targets: rumor.tag_values("e").filter(|v| is_hex_id(v)).take(100).map(str::to_ascii_lowercase).collect() },
        _ => return Err(Error::Wrap("not a chat message")),
    };
    let recipients = rumor.tag_values("p").take(MAX_RECIPIENTS).filter_map(|h| PublicKey::from_hex(h).ok()).collect();
    let subject = rumor.tag_values("subject").next().map(|s| crate::display::clean_line(s, MAX_SUBJECT_CHARS));
    // NIP-17 puts `expiration` on the wrap and the seal; some clients only on the rumor.
    let expires = [wrap.tag_values("expiration").next(), seal.tag_values("expiration").next(), rumor.tag_values("expiration").next()]
        .into_iter()
        .flatten()
        .filter_map(|v| v.trim().parse::<u64>().ok())
        .min();
    Ok(Dm { id: rumor.id_hex(), sender: rumor.pubkey, recipients, created_at: rumor.created_at, subject, kind: rumor.kind, expires, body })
}

fn clip(s: &str) -> String {
    if s.len() <= MAX_SHOWN_BYTES {
        return s.to_string();
    }
    let mut end = MAX_SHOWN_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [cut on Passport]", &s[..end])
}

/// Relay URLs worth signing or showing: `wss://` (or `ws://` for a local test relay), plain
/// ASCII only (internationalised hosts in punycode), so what is on screen is what is signed:
/// no look-alike letters, no invisible or bidi characters. Host: letters, digits, dots and
/// dashes, or a bracketed IPv6 address; then an optional port, path and query.
pub fn valid_relay(url: &str) -> bool {
    if url.len() > 200 {
        return false;
    }
    let Some(rest) = url.strip_prefix("wss://").or_else(|| url.strip_prefix("ws://")) else { return false };
    let (authority, tail) = match rest.find(['/', '?']) {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let Some((h, after)) = v6.split_once(']') else { return false };
        if h.is_empty() || !h.chars().all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.') {
            return false;
        }
        let port = match after {
            "" => None,
            _ => match after.strip_prefix(':') {
                Some(p) => Some(p),
                None => return false,
            },
        };
        (h, port)
    } else {
        let (h, p) = match authority.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (authority, None),
        };
        let label_ok = |l: &str| !l.is_empty() && !l.starts_with('-') && !l.ends_with('-') && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if h.is_empty() || !h.trim_end_matches('.').split('.').all(label_ok) {
            return false;
        }
        (h, p)
    };
    let port_ok = match port {
        None => true,
        Some(p) => !p.is_empty() && p.len() <= 5 && p.chars().all(|c| c.is_ascii_digit()) && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n)),
    };
    let tail_ok = tail.chars().all(|c| c.is_ascii_alphanumeric() || "-._~/?=&%+,;:@!$*()".contains(c));
    !host.is_empty() && port_ok && tail_ok
}

/// NIP-42 challenges are random tokens (hex, UUIDs, base64). Anything else, free text in
/// particular, is not signed.
pub fn valid_challenge(c: &str) -> bool {
    (1..=256).contains(&c.len()) && c.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_:.-".contains(c))
}

/// Kind 10050: "send my DMs to these relays".
pub fn dm_relays_event(me: &SecretKey, relays: &[String], now: u64) -> Result<Event> {
    let tags: Vec<Vec<String>> =
        relays.iter().filter(|r| valid_relay(r)).map(|r| vec!["relay".to_string(), r.clone()]).collect();
    if tags.is_empty() {
        return Err(Error::Frame("no valid relay urls"));
    }
    UnsignedEvent { pubkey: me.public_key(), created_at: now, kind: KIND_DM_RELAYS, tags, content: String::new() }.sign(me)
}

/// Kind 22242 (NIP-42): proves this key to one relay, for one challenge. Inbox relays
/// often require it before they hand out gift wraps addressed to us.
pub fn auth_event(me: &SecretKey, relay: &str, challenge: &str, now: u64) -> Result<Event> {
    if !valid_relay(relay) || !valid_challenge(challenge) {
        return Err(Error::Frame("bad auth request"));
    }
    UnsignedEvent {
        pubkey: me.public_key(),
        created_at: now,
        kind: KIND_AUTH,
        tags: vec![vec!["relay".into(), relay.into()], vec!["challenge".into(), challenge.into()]],
        content: String::new(),
    }
    .sign(me)
}

pub fn is_gift_wrap_for(ev: &Event, me: &PublicKey) -> bool {
    if ev.kind != KIND_GIFT_WRAP || ev.tags.len() > MAX_WRAP_TAGS {
        return false;
    }
    let me = me.to_hex();
    ev.tag_values("p").any(|p| p.eq_ignore_ascii_case(&me))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_and_open_both_copies() {
        let (alice, bob) = (SecretKey::generate().unwrap(), SecretKey::generate().unwrap());
        let out = send_dm(&alice, &bob.public_key(), "gm bob", 1_790_000_000).unwrap();
        assert_eq!(out.wraps.len(), 2);
        let (to_bob, w_bob) = &out.wraps[0];
        let (to_me, w_me) = &out.wraps[1];
        assert_eq!(*to_bob, bob.public_key());
        assert_eq!(*to_me, alice.public_key());
        assert!(is_gift_wrap_for(w_bob, &bob.public_key()));
        assert!(!is_gift_wrap_for(w_bob, &alice.public_key()));

        let at_bob = open_dm(w_bob, &bob).unwrap();
        let at_alice = open_dm(w_me, &alice).unwrap();
        assert_eq!(at_bob, at_alice, "both copies carry the same rumor");
        assert_eq!(at_bob.id, out.rumor.id_hex());
        assert_eq!(at_bob.sender, alice.public_key());
        assert_eq!(at_bob.recipients, vec![bob.public_key()]);
        assert_eq!(at_bob.text(), "gm bob");
        assert_eq!(at_bob.created_at, 1_790_000_000, "the rumor keeps the real time");
    }

    #[test]
    fn note_to_self_has_one_wrap() {
        let me = SecretKey::generate().unwrap();
        let out = send_dm(&me, &me.public_key(), "remember the milk", 1).unwrap();
        assert_eq!(out.wraps.len(), 1);
        assert_eq!(open_dm(&out.wraps[0].1, &me).unwrap().text(), "remember the milk");
    }

    #[test]
    fn text_limits() {
        let me = SecretKey::generate().unwrap();
        assert!(send_dm(&me, &me.public_key(), "   ", 1).is_err());
        assert!(send_dm(&me, &me.public_key(), &"x".repeat(MAX_TEXT_BYTES + 1), 1).is_err());
        assert!(send_dm(&me, &me.public_key(), &"x".repeat(MAX_TEXT_BYTES), 1).is_ok());
        let long = "é".repeat(MAX_SHOWN_BYTES);
        assert!(clip(&long).ends_with("[cut on Passport]"));
    }

    #[test]
    fn relay_list_and_auth() {
        let me = SecretKey::generate().unwrap();
        let ev = dm_relays_event(&me, &["wss://relay.example".into(), "http://nope".into(), "wss://bad url".into()], 7).unwrap();
        ev.verify().unwrap();
        assert_eq!(ev.kind, KIND_DM_RELAYS);
        assert_eq!(ev.tags, vec![vec!["relay".to_string(), "wss://relay.example".to_string()]]);
        assert!(dm_relays_event(&me, &["http://nope".into()], 7).is_err());

        let a = auth_event(&me, "wss://relay.example", "c-123", 9).unwrap();
        a.verify().unwrap();
        assert_eq!(a.kind, KIND_AUTH);
        assert!(auth_event(&me, "https://x", "c", 9).is_err());
        assert!(auth_event(&me, "wss://x.y", "", 9).is_err());
        assert!(auth_event(&me, "wss://x.y", "I authorise a payment", 9).is_err(), "free text is not a challenge");
        assert!(auth_event(&me, "wss://x.y", &"a".repeat(257), 9).is_err());
        for ok in ["5f2b1c9e", "3c1c3d02-4b0e-4b5a-9d0e-7f1f8a2b6c11", "q83v+Zx/8w==", "abc_DEF-12.3:4"] {
            assert!(auth_event(&me, "wss://x.y", ok, 9).is_ok(), "{ok}");
        }
    }

    #[test]
    fn relay_url_grammar() {
        for ok in [
            "wss://relay.damus.io",
            "wss://nos.lol/",
            "ws://localhost:7778",
            "ws://127.0.0.1:7778",
            "wss://[::1]:443/x",
            "wss://[2001:db8::1]",
            "wss://filter.nostr.wine/npub1abc?broadcast=true",
            "wss://relay.example.com.",
            "wss://xn--80ak6aa92e.com",
            "wss://RELAY.EXAMPLE",
        ] {
            assert!(valid_relay(ok), "{ok} refused");
        }
        for bad in [
            "wss://",
            "wss://:80",
            "wss://a:",
            "wss://a:0",
            "wss://a:99999",
            "wss://a b",
            "wss://relay.damus.io\u{202E}",
            "wss://relay.da\u{200B}mus.io",
            "wss://relay.d\u{0430}mus.io",
            "wss://user@host",
            "wss://-a.com",
            "wss://a..b",
            "wss://[zz]",
            "wss://[::1]x",
            "wss://a.com/<script>",
            "https://a.com",
            "wss://a.com/\"",
        ] {
            assert!(!valid_relay(bad), "{bad:?} accepted");
        }
        assert!(!valid_relay(&format!("wss://{}.com", "a".repeat(200))));
    }
}
