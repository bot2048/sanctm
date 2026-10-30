// SPDX-License-Identifier: GPL-3.0-or-later

//! Everything the Passport app does with keys, messages and link frames, without any UI or
//! KeyOS code, so it runs the same in the app, in the headless test device and in tests.
//!
//! The app feeds it link events (`link_changed`, `handle_frame`) and user actions (`send`,
//! contacts) and applies the returned [`Effects`]: frames to send, state to persist and
//! redraw, arrivals to buzz about.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::display::{clean_line, clean_text, is_spoofing};
use crate::inbox::{Inbox, Keep, Message, Pending, Status};
use crate::link::{self, Inbound};
use crate::nip17::{self, is_gift_wrap_for};
use crate::nip59::TIME_JITTER;
use crate::profile::{self, Profiles, AVATAR_PX};
use crate::{Error, Event, Identity, PublicKey, Result};

pub const MAX_NAME_CHARS: usize = 24;
pub const MAX_CONTACTS: usize = 500;
/// Before this the system clock is clearly unset; use the companion's clock instead.
const SANE_CLOCK: u64 = 1_700_000_000;
/// A companion clock past this (2100-01-01) is wrong, e.g. milliseconds.
const MAX_CLOCK: u64 = 4_102_444_800;
/// Rumors stamped further ahead than this are shown at "now".
const MAX_FUTURE_SKEW: u64 = 600;
/// NIP-42: at most this many AUTH signatures per relay within `AUTH_WINDOW` seconds.
/// Measured on the monotonic clock: the wall clock can be set by the companion.
const AUTH_BURST: usize = 2;
const AUTH_TOTAL: usize = 8;
const AUTH_WINDOW: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub name: String,
    #[serde(with = "crate::event::pubkey_hex")]
    pub pk: PublicKey,
}

pub fn validate_name(name: &str) -> Result<String> {
    let name = name.trim();
    let n = name.chars().count();
    if n == 0 || n > MAX_NAME_CHARS {
        return Err(Error::Frame("name must be 1 to 24 characters"));
    }
    if name.chars().any(|c| c.is_control() || is_spoofing(c)) {
        return Err(Error::Frame("name has invisible or direction characters"));
    }
    Ok(name.to_string())
}

/// A name folded to what it looks like on screen: lowercase, with the Cyrillic and Greek
/// letters that are drawn like Latin ones mapped to them (a short confusables table).
fn skeleton(name: &str) -> String {
    name.chars()
        .flat_map(char::to_lowercase)
        .map(|c| match c {
            'а' | 'α' => 'a',
            'в' | 'β' => 'b',
            'с' | 'ϲ' => 'c',
            'ԁ' => 'd',
            'е' | 'ё' | 'ε' => 'e',
            'һ' | 'н' | 'η' => 'h',
            'i' | 'і' | 'ї' | 'ι' | 'ı' | '1' | 'l' | '|' => 'l',
            'ј' => 'j',
            'к' | 'κ' => 'k',
            'м' | 'μ' => 'm',
            'п' => 'n',
            'ν' => 'v',
            'о' | 'ο' | 'σ' | '0' => 'o',
            'р' | 'ρ' => 'p',
            'ԛ' => 'q',
            'ѕ' => 's',
            'т' | 'τ' => 't',
            'υ' => 'u',
            'х' | 'χ' => 'x',
            'у' | 'γ' => 'y',
            'ᴢ' | 'ζ' => 'z',
            c => c,
        })
        .filter(|c| !c.is_whitespace())
        .collect()
}

pub struct Arrival {
    pub peer: PublicKey,
    pub text: String,
    pub known: bool,
}

#[derive(Default)]
pub struct Effects {
    /// Frames for the link, in order.
    pub frames: Vec<String>,
    /// Inbox or contacts changed: persist and redraw.
    pub changed: bool,
    /// Only link / sync / relay status changed: redraw.
    pub status: bool,
    pub arrived: Vec<Arrival>,
    /// Wraps addressed to us that did not open (forged, damaged, or another app's).
    pub refused: u32,
    /// A NIP-05 lookup finished: (address, key or why not).
    pub nip05: Option<(String, std::result::Result<PublicKey, String>)>,
    /// Pictures that finished loading (not stored unless kept).
    pub pictures: Vec<Picture>,
}

/// A picture in a message, as RGBA pixels ready to show.
pub struct Picture {
    /// Rumor id of the message it belongs to.
    pub msg: String,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// Why it did not load (then `rgba` is empty).
    pub error: Option<String>,
}

/// Biggest picture the phone may send, in pixels: about the width of a chat bubble.
pub const PICTURE_W: u32 = 360;
pub const PICTURE_H: u32 = 480;
/// Biggest encrypted file the Passport fetches (bytes).
pub const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;

enum FetchFor {
    Avatar { pk: PublicKey },
    Picture { msg: String },
    File { msg: String },
}

struct Fetch {
    what: FetchFor,
    url: String,
    buf: Vec<u8>,
    next: u32,
    max: usize,
    w: u32,
    h: u32,
}

impl Effects {
    fn merge(&mut self, o: Effects) {
        self.frames.extend(o.frames);
        self.changed |= o.changed;
        self.status |= o.status;
        self.arrived.extend(o.arrived);
        self.refused += o.refused;
        if o.nip05.is_some() {
            self.nip05 = o.nip05;
        }
        self.pictures.extend(o.pictures);
    }
}

pub struct Session {
    pub me: Identity,
    pub contacts: Vec<Contact>,
    pub inbox: Inbox,
    pub link_up: bool,
    pub synced: bool,
    pub companion: String,
    /// The companion's relays: what a published kind 10050 would list.
    pub relays: Vec<String>,
    /// The relays the user confirmed by publishing them as DM relays (persisted by the app).
    /// Once there are some, AUTH is signed for these only: the phone names `relays` itself.
    pub auth_relays: Vec<String>,
    pub relays_up: u32,
    pub relays_total: u32,
    clock_offset: i64,
    fixed_now: Option<u64>,
    /// (relay, when) of recent AUTH signatures, for the rate limit.
    auth_log: Vec<(String, Instant)>,
    /// Nostr profiles of the people we chat with (persisted by the app).
    pub profiles: Profiles,
    /// Fetch and show profiles (names, pictures, NIP-05). The phone then learns whose
    /// profiles they are, so it can be turned off.
    pub show_profiles: bool,
    asked_profiles: HashSet<String>,
    nip05_pending: Option<String>,
    fetches: HashMap<String, Fetch>,
}

/// What the phone says went wrong, shown on the Passport only as one of a few fixed phrases.
fn shuffle(v: &mut [String]) {
    let mut seed = [0u8; 8];
    if getrandom::getrandom(&mut seed).is_err() {
        return;
    }
    let mut x = u64::from_le_bytes(seed) | 1;
    for i in (1..v.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

fn known_reason(said: Option<&str>, known: &[&str], otherwise: &str) -> String {
    let said = said.unwrap_or("").to_ascii_lowercase();
    if let Some(k) = known.iter().find(|k| said.contains(*k)) {
        return (*k).to_string();
    }
    if said.contains("the server said") {
        if let Some(code) = said.split_whitespace().find(|w| w.len() == 3 && w.chars().all(|c| c.is_ascii_digit())) {
            return format!("the server said {code}");
        }
    }
    otherwise.to_string()
}

fn system_now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) }

impl Session {
    pub fn new(me: Identity, contacts: Vec<Contact>, inbox: Inbox) -> Self {
        Self {
            me,
            contacts,
            inbox,
            link_up: false,
            synced: false,
            companion: String::new(),
            relays: Vec::new(),
            auth_relays: Vec::new(),
            relays_up: 0,
            relays_total: 0,
            clock_offset: 0,
            fixed_now: None,
            auth_log: Vec::new(),
            profiles: Profiles::default(),
            show_profiles: true,
            asked_profiles: HashSet::new(),
            nip05_pending: None,
            fetches: HashMap::new(),
        }
    }

    /// Drop messages that expired (NIP-40). Call now and then; true if any went.
    pub fn tick(&mut self) -> bool {
        let now = self.now();
        now >= SANE_CLOCK && self.inbox.purge_expired(now)
    }

    /// Tests only: freeze the clock.
    pub fn set_now(&mut self, now: u64) { self.fixed_now = Some(now); }

    pub fn now(&self) -> u64 {
        if let Some(t) = self.fixed_now {
            return t;
        }
        system_now().saturating_add_signed(self.clock_offset)
    }

    /// Subscription start: back to the last sync, minus the NIP-59 timestamp jitter.
    pub fn since(&self) -> u64 {
        if self.inbox.last_sync == 0 {
            0
        } else {
            self.inbox.last_sync.saturating_sub(TIME_JITTER + 3600)
        }
    }

    pub fn contact(&self, pk: &PublicKey) -> Option<&Contact> { self.contacts.iter().find(|c| c.pk == *pk) }

    /// Your name for them, else their Nostr profile's name, else a short npub.
    pub fn display_name(&self, pk: &PublicKey) -> String {
        if *pk == self.me.pk {
            return "Note to self".into();
        }
        if let Some(c) = self.contact(pk) {
            return c.name.clone();
        }
        self.profile_label(pk).unwrap_or_else(|| crate::nip19::short_npub(pk))
    }

    /// Name (or NIP-05) from their Nostr profile, if profiles are on and we have it.
    pub fn profile_label(&self, pk: &PublicKey) -> Option<String> {
        if !self.show_profiles {
            return None;
        }
        self.profiles.get(pk).and_then(|p| p.label())
    }

    pub fn profile(&self, pk: &PublicKey) -> Option<&profile::Profile> {
        if self.show_profiles {
            self.profiles.get(pk)
        } else {
            None
        }
    }

    // ---- link -------------------------------------------------------------------------

    pub fn link_changed(&mut self, up: bool) -> Effects {
        let mut fx = Effects { status: true, ..Effects::default() };
        self.link_up = up;
        self.synced = false;
        self.asked_profiles.clear();
        self.fetches.clear();
        self.nip05_pending = None;
        if up {
            fx.frames.push(link::hello(&self.me.pk, self.since()));
            fx.merge(self.flush_outbox());
        } else {
            self.relays_up = 0;
            let sending: Vec<String> = self
                .inbox
                .outbox
                .iter()
                .filter(|p| p.primary)
                .map(|p| p.rumor.clone())
                .collect();
            for id in sending {
                if self.inbox.find_message(&id).map(|m| m.status == Status::Sending).unwrap_or(false) {
                    fx.changed |= self.inbox.set_status(&id, Status::Queued);
                }
            }
        }
        fx
    }

    fn flush_outbox(&mut self) -> Effects {
        let mut fx = Effects::default();
        let items = self.inbox.outbox.clone();
        for p in &items {
            let to = PublicKey::from_hex(&p.to).ok();
            fx.frames.push(link::publish(&p.wrap, to.as_ref()));
            if p.primary {
                if let Some(m) = self.inbox.find_message(&p.rumor) {
                    if matches!(m.status, Status::Queued | Status::Failed { .. }) {
                        fx.changed |= self.inbox.set_status(&p.rumor, Status::Sending);
                    }
                }
            }
        }
        fx
    }

    pub fn handle_frame(&mut self, text: &str) -> Effects {
        let mut fx = Effects::default();
        let frame = match link::parse(text) {
            Ok(f) => f,
            Err(_) => return fx,
        };
        match frame {
            Inbound::Hello { client, now, relays, .. } => {
                self.companion = clean_line(&client, 40);
                self.relays = relays.into_iter().filter(|r| nip17::valid_relay(r)).take(12).collect();
                if let Some(now) = now.filter(|n| (SANE_CLOCK..MAX_CLOCK).contains(n)) {
                    let sys = system_now();
                    if sys < SANE_CLOCK {
                        self.clock_offset = now as i64 - sys as i64; // both < 2^33
                    }
                }
                fx.status = true;
            }
            Inbound::Event { ev, .. } => fx.merge(self.handle_wrap(ev)),
            Inbound::Eose {} => {
                self.synced = true;
                self.inbox.last_sync = self.now();
                self.inbox.purge_expired(self.now());
                fx.changed = true;
                fx.status = true;
                let keys = self.profile_keys();
                // Forget profiles of people we no longer chat with (deleted chats, removed contacts).
                let keep: HashSet<&String> = keys.iter().collect();
                self.profiles.retain(|k| keep.contains(&k.to_string()));
                let auto = self.auto_profile_keys();
                fx.merge(self.ask_profiles(&auto));
                fx.merge(self.fetch_missing_avatars());
            }
            Inbound::Ok { id, accepted, failed } => fx.merge(self.handle_ok(&id, accepted.len() as u32, failed)),
            Inbound::Auth { relay, challenge } => {
                if let Some(ev) = self.answer_auth(&relay, &challenge) {
                    fx.frames.push(link::auth(&ev));
                }
            }
            Inbound::Relays { up, total } => {
                self.relays_up = up;
                self.relays_total = total;
                fx.status = true;
            }
            Inbound::Ping {} => fx.frames.push(link::pong()),
            Inbound::Pong {} => {}
            Inbound::Profile { ev, nip05_ok } => fx.merge(self.handle_profile(ev, nip05_ok)),
            Inbound::Nip05 { q, pubkey, error } => {
                if self.nip05_pending.as_deref() == Some(q.as_str()) {
                    self.nip05_pending = None;
                    let r = match pubkey.as_deref().map(PublicKey::from_hex) {
                        Some(Ok(pk)) => Ok(pk),
                        _ => Err(known_reason(error.as_deref(), &["not found", "timed out", "no such domain, or it refused"], "lookup failed")),
                    };
                    if let Ok(pk) = &r {
                        fx.merge(self.ask_profiles(&[pk.to_hex()]));
                    }
                    fx.nip05 = Some((q, r));
                }
            }
            Inbound::Blob { id, seq, last, w, h, data, error } => fx.merge(self.handle_blob(&id, seq, last, w, h, &data, error)),
        }
        fx
    }

    fn handle_wrap(&mut self, raw: serde_json::Value) -> Effects {
        let mut fx = Effects::default();
        let Ok(ev) = serde_json::from_value::<Event>(raw) else { return fx };
        if !is_gift_wrap_for(&ev, &self.me.pk) {
            return fx;
        }
        let wrap_id = ev.id_hex();
        if self.inbox.has_wrap(&wrap_id) {
            return fx;
        }
        // Only a wrap whose id and signature check out may claim its id: a damaged copy must
        // not keep the genuine one (same id, from another relay) out.
        if ev.verify().is_err() {
            fx.refused += 1;
            return fx;
        }
        self.inbox.note_wrap(&wrap_id);
        fx.changed = true;
        let dm = match nip17::open_dm(&ev, &self.me.sk) {
            Ok(dm) => dm,
            Err(_) => {
                fx.refused += 1;
                return fx;
            }
        };
        let me = self.me.pk;
        let out = dm.sender == me;
        let now = self.now();
        if dm.expires.is_some_and(|e| now >= SANE_CLOCK && e <= now) {
            return fx; // already expired (NIP-40): never shown
        }
        let (text, file, reply_to) = match dm.body {
            nip17::Body::Reaction { target, emoji } => {
                // Only the other person in that chat (or we, from another device) react to it;
                // a reaction for a message not here yet waits, and is checked when it arrives.
                let by = dm.sender.to_hex();
                let allowed = match self.inbox.chat_of(&target) {
                    Some(peer) => out || peer == by,
                    None => true,
                };
                if allowed {
                    self.inbox.react(&target, crate::inbox::Reaction { by, emoji, id: dm.id.clone(), at: dm.created_at });
                    let (me_hex, contacts) = (me.to_hex(), self.contact_keys());
                    self.inbox.make_room(&Keep { me: Some(&me_hex), contacts: &contacts });
                }
                return fx;
            }
            nip17::Body::Delete { targets } => {
                self.inbox.delete_by(&targets, &dm.sender.to_hex(), out);
                return fx;
            }
            nip17::Body::Message { text, file, reply_to } => (text, file, reply_to),
        };
        let others: Vec<PublicKey> = dm.recipients.iter().copied().filter(|p| *p != me && *p != dm.sender).collect();
        // Our own copy belongs to the chat with the recipient; an incoming one to the sender.
        let peer = if out { others.first().copied().unwrap_or(me) } else { dm.sender };
        let group = if out { others.len() > 1 } else { !others.is_empty() };
        let body = clean_text(&text);
        let text = match &dm.subject {
            Some(s) if !s.is_empty() && body.is_empty() => format!("[{s}]"),
            Some(s) if !s.is_empty() => format!("[{s}] {body}"),
            _ => body,
        };
        let at = if now >= SANE_CLOCK && dm.created_at > now + MAX_FUTURE_SKEW { now } else { dm.created_at };
        let shown = match &file {
            Some(f) if text.is_empty() => f.label().to_string(),
            Some(f) => format!("{}: {text}", f.label()),
            None => text.clone(),
        };
        let mut msg = Message::new(dm.id.clone(), out, text, at, if out { Status::Sent { relays: 1 } } else { Status::Received });
        msg.group = group;
        msg.reply_to = reply_to;
        msg.expires = dm.expires;
        msg.file = file;
        let (me_hex, contacts) = (me.to_hex(), self.contact_keys());
        let keep = Keep { me: Some(&me_hex), contacts: &contacts };
        let peer_hex = peer.to_hex();
        if self.inbox.add_protected(&peer_hex, msg, keep) && !out {
            fx.arrived.push(Arrival { peer, text: shown, known: self.contact(&peer).is_some() });
        }
        fx
    }

    fn contact_keys(&self) -> Vec<String> { self.contacts.iter().map(|c| c.pk.to_hex()).collect() }

    // ---- profiles, NIP-05, pictures ------------------------------------------------------

    /// Profiles fetched without asking: ours, contacts', and those of people we wrote to (the
    /// phone already knows them as recipients). Strangers' profiles are fetched when their chat
    /// is opened (`ask_profiles`), so a stranger's message alone tells the phone nothing.
    pub fn auto_profile_keys(&self) -> Vec<String> {
        let mut v: Vec<String> = std::iter::once(self.me.pk.to_hex())
            .chain(self.contact_keys())
            .chain(self.inbox.chats.iter().filter(|c| c.messages.iter().any(|m| m.out)).map(|c| c.peer.clone()))
            .collect();
        v.sort();
        v.dedup();
        v
    }

    /// Whose profiles we keep: ourselves, contacts, everyone we have a chat with.
    pub fn profile_keys(&self) -> Vec<String> {
        let mut v: Vec<String> = std::iter::once(self.me.pk.to_hex()).chain(self.contact_keys()).chain(self.inbox.chats.iter().map(|c| c.peer.clone())).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Ask the phone for these profiles (once per link), if profiles are on.
    pub fn ask_profiles(&mut self, keys: &[String]) -> Effects {
        let mut fx = Effects::default();
        if !self.show_profiles || !self.link_up {
            return fx;
        }
        let mut new: Vec<String> = keys.iter().filter(|k| PublicKey::from_hex(k).is_ok() && self.asked_profiles.insert((*k).clone())).cloned().collect();
        // Our own profile on its own, the others in small shuffled batches: one relay request
        // naming us next to everyone we talk to would undo what gift wraps hide.
        let me = self.me.pk.to_hex();
        if let Some(i) = new.iter().position(|k| *k == me) {
            fx.frames.push(link::want_profiles(&[new.remove(i)]));
        }
        shuffle(&mut new);
        for chunk in new.chunks(3) {
            fx.frames.push(link::want_profiles(chunk));
        }
        fx
    }

    /// Ask the phone for avatars we do not have yet (e.g. after profiles are switched back on).
    pub fn fetch_missing_avatars(&mut self) -> Effects {
        let mut fx = Effects::default();
        if !self.show_profiles || !self.link_up {
            return fx;
        }
        for (hex, url) in self.profiles.missing_avatars().into_iter().filter(|(k, _)| self.asked_profiles.contains(k)).take(60) {
            let Ok(pk) = PublicKey::from_hex(&hex) else { continue };
            let id = format!("av:{hex}");
            if self.fetches.contains_key(&id) {
                continue;
            }
            let max = (AVATAR_PX * AVATAR_PX * 4) as usize;
            fx.frames.push(link::fetch(&id, &url, "avatar", AVATAR_PX, AVATAR_PX, max));
            self.fetches.insert(id, Fetch { what: FetchFor::Avatar { pk }, url, buf: Vec::new(), next: 0, max, w: AVATAR_PX, h: AVATAR_PX });
        }
        fx
    }

    fn handle_profile(&mut self, raw: serde_json::Value, nip05_ok: bool) -> Effects {
        let mut fx = Effects::default();
        if !self.show_profiles {
            return fx;
        }
        let Ok(ev) = serde_json::from_value::<Event>(raw) else { return fx };
        let hex = ev.pubkey.to_hex();
        // Only profiles we asked for: the phone cannot fill the Passport with strangers'.
        if !self.asked_profiles.contains(&hex) {
            return fx;
        }
        let Ok(p) = profile::parse(&ev) else { return fx };
        if self.profiles.put(&ev.pubkey, p, nip05_ok) {
            fx.changed = true;
            fx.merge(self.fetch_missing_avatars());
        }
        fx
    }

    /// Resolve `name@domain` through the phone. The answer arrives as `Effects::nip05`.
    pub fn lookup_nip05(&mut self, input: &str) -> Result<Effects> {
        let q = profile::parse_nip05(input).ok_or(Error::Frame("not a name@domain address"))?;
        if !self.link_up {
            return Err(Error::Frame("looking up a name needs the phone linked"));
        }
        self.nip05_pending = Some(q.clone());
        Ok(Effects { frames: vec![link::nip05(&q)], ..Effects::default() })
    }

    /// Fetch the picture at `url` in message `msg` (a link in its text), through the phone.
    pub fn load_picture(&mut self, msg: &str, url: &str) -> Result<Effects> {
        let url = profile::https_url(url);
        if url.is_empty() {
            return Err(Error::Frame("not a picture link"));
        }
        if !self.link_up {
            return Err(Error::Frame("loading a picture needs the phone linked"));
        }
        let id = format!("pic:{msg}");
        let max = (PICTURE_W * PICTURE_H * 4) as usize;
        self.fetches.insert(id.clone(), Fetch { what: FetchFor::Picture { msg: msg.into() }, url: url.clone(), buf: Vec::new(), next: 0, max, w: 0, h: 0 });
        Ok(Effects { frames: vec![link::fetch(&id, &url, "picture", PICTURE_W, PICTURE_H, max)], ..Effects::default() })
    }

    /// Fetch the encrypted file of a kind 15 message, to open it here.
    pub fn load_file(&mut self, msg: &str) -> Result<Effects> {
        let Some(m) = self.inbox.find_message(msg) else { return Err(Error::Frame("no such message")) };
        let Some(f) = m.file.as_ref() else { return Err(Error::Frame("not a file message")) };
        if !self.link_up {
            return Err(Error::Frame("loading a file needs the phone linked"));
        }
        let url = profile::https_url(&f.url);
        if url.is_empty() {
            return Err(Error::Frame("the file link is not https"));
        }
        if f.size.is_some_and(|s| s > MAX_FILE_BYTES as u64) {
            return Err(Error::Frame("too big to open on Passport"));
        }
        let id = format!("file:{msg}");
        self.fetches.insert(id.clone(), Fetch { what: FetchFor::File { msg: msg.into() }, url: url.clone(), buf: Vec::new(), next: 0, max: MAX_FILE_BYTES, w: 0, h: 0 });
        Ok(Effects { frames: vec![link::fetch(&id, &url, "file", 0, 0, MAX_FILE_BYTES)], ..Effects::default() })
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_blob(&mut self, id: &str, seq: u32, last: bool, w: u32, h: u32, data: &str, error: Option<String>) -> Effects {
        let mut fx = Effects::default();
        let Some(f) = self.fetches.get_mut(id) else { return fx };
        let fail = |what: &FetchFor, why: String, fx: &mut Effects| {
            if let FetchFor::Picture { msg } | FetchFor::File { msg } = what {
                fx.pictures.push(Picture { msg: msg.clone(), w: 0, h: 0, rgba: Vec::new(), error: Some(why) });
            }
        };
        if let Some(e) = error {
            let f = self.fetches.remove(id).expect("present");
            let why = known_reason(Some(&e), &["too big", "timed out", "not a picture", "the server does not allow this"], "could not load");
            fail(&f.what, why, &mut fx);
            return fx;
        }
        let chunk = match profile::decode_rgba(data) {
            Ok(c) if seq == f.next && f.buf.len() + c.len() <= f.max => c,
            _ => {
                let f = self.fetches.remove(id).expect("present");
                fail(&f.what, "the phone sent a damaged file".into(), &mut fx);
                return fx;
            }
        };
        f.buf.extend_from_slice(&chunk);
        f.next += 1;
        if seq == 0 {
            f.w = w;
            f.h = h;
        }
        if !last {
            return fx;
        }
        let f = self.fetches.remove(id).expect("present");
        match f.what {
            FetchFor::Avatar { pk } => {
                if self.profiles.set_avatar(&pk, &f.url, f.buf) {
                    fx.changed = true;
                }
            }
            FetchFor::Picture { msg } => {
                let ok = f.w > 0 && f.h > 0 && f.w <= PICTURE_W && f.h <= PICTURE_H && f.buf.len() == (f.w * f.h * 4) as usize;
                if ok {
                    fx.pictures.push(Picture { msg, w: f.w, h: f.h, rgba: f.buf, error: None });
                } else {
                    fx.pictures.push(Picture { msg, w: 0, h: 0, rgba: Vec::new(), error: Some("not a picture".into()) });
                }
            }
            FetchFor::File { msg } => {
                let r = self.inbox.find_message(&msg).and_then(|m| m.file.clone()).ok_or(Error::Frame("message gone")).and_then(|meta| crate::media::open_file(&meta, &f.buf));
                match r {
                    Ok(img) => fx.pictures.push(Picture { msg, w: img.w, h: img.h, rgba: img.rgba, error: None }),
                    // media.rs reasons are written for the screen; drop the "link frame: " prefix.
                    Err(Error::Frame(why)) => fx.pictures.push(Picture { msg, w: 0, h: 0, rgba: Vec::new(), error: Some(why.to_string()) }),
                    Err(e) => fx.pictures.push(Picture { msg, w: 0, h: 0, rgba: Vec::new(), error: Some(e.to_string()) }),
                }
            }
        }
        fx
    }

    /// NIP-42 AUTH, signed only for the relays the companion listed as ours in its hello
    /// (signing for a recipient's inbox relay would tell it who sent the gift wrap), only for
    /// nonce-like challenges, and at most `AUTH_BURST` times per relay per minute. The
    /// companion is untrusted; this keeps it from using the Passport as a login signer.
    fn answer_auth(&mut self, relay: &str, challenge: &str) -> Option<Event> {
        let ours = if self.auth_relays.is_empty() { &self.relays } else { &self.auth_relays };
        if !ours.iter().any(|r| r == relay) || !nip17::valid_challenge(challenge) {
            return None;
        }
        self.auth_log.retain(|(_, t)| t.elapsed() < AUTH_WINDOW);
        // Per relay, and in total: a phone that keeps announcing new relays gets no more.
        if self.auth_log.iter().filter(|(r, _)| r == relay).count() >= AUTH_BURST || self.auth_log.len() >= AUTH_TOTAL {
            return None;
        }
        let ev = nip17::auth_event(&self.me.sk, relay, challenge, self.now()).ok()?;
        self.auth_log.push((relay.to_string(), Instant::now()));
        Some(ev)
    }

    fn handle_ok(&mut self, wrap_id: &str, accepted: u32, failed: Vec<String>) -> Effects {
        let mut fx = Effects::default();
        let Some(pos) = self.inbox.outbox.iter().position(|p| p.wrap.id_hex() == wrap_id) else { return fx };
        let p = self.inbox.outbox[pos].clone();
        if accepted > 0 {
            self.inbox.outbox.remove(pos);
            fx.changed = true;
            if p.primary {
                self.inbox.set_status(&p.rumor, Status::Sent { relays: accepted });
            }
        } else if p.primary {
            // Shown on the Passport's screen and kept with the message: one of a few fixed
            // phrases (NIP-01 OK prefixes), never the phone's own words ("SECURITY ALERT ...").
            // Each entry is "host: reason"; only the reason is read (a host name like
            // "pow.example" must not pick the phrase). A relay's own answer ("blocked") says more
            // than one that never answered, so it wins over "unreachable" / "timeout", whatever
            // the order the phone listed them in.
            const VAGUE: [&str; 5] = ["unreachable", "timeout", "timed out", "no relay reachable", "a relay refused it"];
            let reasons: Vec<String> = failed
                .iter()
                .map(|w| {
                    let said = match w.split_once(": ") {
                        Some((host, r)) if !host.contains(char::is_whitespace) && host.contains(['.', ':', '/']) => r,
                        _ => w.as_str(),
                    };
                    known_reason(
                        Some(said),
                        &["auth-required", "rate-limited", "blocked", "restricted", "duplicate", "invalid", "pow", "timed out", "timeout", "unreachable", "no relay reachable"],
                        "a relay refused it",
                    )
                })
                .collect();
            let why = reasons
                .iter()
                .find(|r| !VAGUE.contains(&r.as_str()))
                .or_else(|| reasons.first())
                .cloned()
                .unwrap_or_else(|| "no relay reachable".to_string());
            fx.changed |= self.inbox.set_status(&p.rumor, Status::Failed { why });
        }
        fx
    }

    // ---- user actions -----------------------------------------------------------------

    /// Seal, wrap (for them and for us) and queue a message. Goes out now if a phone is linked.
    pub fn send(&mut self, to: &PublicKey, text: &str) -> Result<Effects> { self.send_message(to, text, None) }

    /// A reply to message `parent` (rumor id) in the chat with `to`.
    pub fn send_reply(&mut self, to: &PublicKey, text: &str, parent: &str) -> Result<Effects> { self.send_message(to, text, Some(parent)) }

    /// Publish a failed message again now (the user asked; otherwise it waits for the next link).
    pub fn retry(&mut self, rumor: &str) -> Effects {
        let mut fx = Effects::default();
        let failed = self.inbox.find_message(rumor).is_some_and(|m| matches!(m.status, Status::Failed { .. }));
        if !self.link_up || !failed {
            return fx;
        }
        for p in self.inbox.outbox.iter().filter(|p| p.rumor == rumor) {
            let to = PublicKey::from_hex(&p.to).ok();
            fx.frames.push(link::publish(&p.wrap, to.as_ref()));
        }
        fx.changed |= self.inbox.set_status(rumor, Status::Sending);
        fx
    }

    /// React to message `target` in the chat with `peer` (one reaction per message: a new one
    /// replaces ours). Sent like a message, to them and to ourselves.
    pub fn react(&mut self, peer: &PublicKey, target: &str, emoji: &str) -> Result<Effects> {
        let (author, kind) = {
            let chat = self.inbox.chat(&peer.to_hex()).ok_or(Error::Frame("no such chat"))?;
            let m = chat.messages.iter().find(|m| m.id == target).ok_or(Error::Frame("no such message"))?;
            (if m.out { self.me.pk } else { *peer }, if m.file.is_some() { nip17::KIND_FILE } else { nip17::KIND_CHAT })
        };
        let now = self.now();
        let me_hex = self.me.pk.to_hex();
        // Our earlier reaction to it: other clients keep every kind 7, so take it back (kind 5).
        let old = self.inbox.find_message(target).and_then(|m| m.reactions.iter().find(|r| r.by == me_hex)).map(|r| r.id.clone()).filter(|id| !id.is_empty());
        let rumor = nip17::reaction_rumor(&self.me.pk, peer, target, kind, &author, emoji, now)?;
        let mut outs = vec![nip17::wrap_for(&self.me.sk, peer, rumor, now)?];
        if let Some(old) = &old {
            outs.push(nip17::wrap_for(&self.me.sk, peer, nip17::delete_rumor(&self.me.pk, peer, std::slice::from_ref(old), nip17::KIND_REACTION, now)?, now)?);
        }
        let rumor_id = outs[0].rumor.id_hex();
        self.inbox.react(target, crate::inbox::Reaction { by: me_hex.clone(), emoji: emoji.trim().to_string(), id: rumor_id.clone(), at: now });
        if let Some(old) = old {
            self.inbox.delete_by(&[old], &me_hex, true);
        }
        let mut fx = Effects { changed: true, ..Effects::default() };
        for out in outs {
            let id = out.rumor.id_hex();
            for (rcpt, wrap) in out.wraps {
                if self.link_up {
                    fx.frames.push(link::publish(&wrap, Some(&rcpt)));
                }
                if rcpt == self.me.pk {
                    self.inbox.note_wrap(&wrap.id_hex());
                }
                self.inbox.outbox.push(Pending { wrap, to: rcpt.to_hex(), rumor: id.clone(), primary: false });
            }
        }
        Ok(fx)
    }

    fn send_message(&mut self, to: &PublicKey, text: &str, parent: Option<&str>) -> Result<Effects> {
        let mut now = self.now();
        // The same text to the same chat in the same second would be the same rumor (same id)
        // and silently merge with the first: nudge the time so it is a second message.
        let peer = to.to_hex();
        while self.inbox.chat(&peer).is_some_and(|c| c.messages.iter().any(|m| m.out && m.at == now && m.text == text)) {
            now += 1;
        }
        let out = match parent {
            Some(p) => nip17::wrap_for(&self.me.sk, to, nip17::reply_rumor(&self.me.pk, to, text, p, now)?, now)?,
            None => nip17::send_dm(&self.me.sk, to, text, now)?,
        };
        let rumor_id = out.rumor.id_hex();
        let mut fx = Effects { changed: true, ..Effects::default() };
        let (me_hex, contacts) = (self.me.pk.to_hex(), self.contact_keys());
        let mut msg = Message::new(rumor_id.clone(), true, text.to_string(), now, if self.link_up { Status::Sending } else { Status::Queued });
        msg.reply_to = parent.map(str::to_ascii_lowercase);
        self.inbox.add_protected(&to.to_hex(), msg, Keep { me: Some(&me_hex), contacts: &contacts });
        for (i, (rcpt, wrap)) in out.wraps.into_iter().enumerate() {
            if self.link_up {
                fx.frames.push(link::publish(&wrap, Some(&rcpt)));
            }
            if rcpt == self.me.pk {
                self.inbox.note_wrap(&wrap.id_hex()); // its echo from our inbox relays adds nothing
            }
            self.inbox.outbox.push(Pending { wrap, to: rcpt.to_hex(), rumor: rumor_id.clone(), primary: i == 0 });
        }
        Ok(fx)
    }

    /// Sign and publish a kind 10050 listing the companion's relays, so other clients know
    /// where to deliver our DMs. `shown` is the list the user saw and confirmed: if the
    /// companion changed it since, nothing is signed.
    pub fn publish_dm_relays(&mut self, shown: &[String]) -> Result<Effects> {
        if !self.link_up {
            return Err(Error::Frame("no phone linked"));
        }
        if self.relays.is_empty() || self.relays != shown {
            return Err(Error::Frame("the relay list changed; check it and try again"));
        }
        let ev = nip17::dm_relays_event(&self.me.sk, &self.relays, self.now())?;
        self.auth_relays = self.relays.clone();
        Ok(Effects { frames: vec![link::publish(&ev, None)], ..Effects::default() })
    }

    pub fn save_contact(&mut self, pk: PublicKey, name: &str) -> Result<()> {
        let name = validate_name(name)?;
        // Compared as they look, not as they are spelled: "Bоb" (Cyrillic о), pre-filled from
        // a stranger's profile, must not become a second "Bob".
        if self.contacts.iter().any(|c| c.pk != pk && skeleton(&c.name) == skeleton(&name)) {
            return Err(Error::Frame("another contact already has that name"));
        }
        match self.contacts.iter_mut().find(|c| c.pk == pk) {
            Some(c) => c.name = name,
            None => {
                if self.contacts.len() >= MAX_CONTACTS {
                    return Err(Error::Frame("contact list is full"));
                }
                self.contacts.push(Contact { name, pk });
                self.contacts.sort_by_key(|c| c.name.to_lowercase());
            }
        }
        Ok(())
    }

    pub fn remove_contact(&mut self, pk: &PublicKey) { self.contacts.retain(|c| c.pk != *pk); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SecretKey;

    fn session() -> Session {
        let mut s = Session::new(Identity::from_secret(SecretKey::generate().unwrap()), vec![], Inbox::default());
        s.set_now(1_790_000_000);
        s
    }

    fn frames_of(fx: &Effects, t: &str) -> Vec<serde_json::Value> {
        fx.frames.iter().map(|f| serde_json::from_str::<serde_json::Value>(f).unwrap()).filter(|v| v["t"] == t).collect()
    }

    /// What a relay would hand the other side: the published wrap, as an inbound frame.
    fn deliver(wrap: &serde_json::Value) -> String { serde_json::json!({"t": "event", "ev": wrap, "relay": "wss://r"}).to_string() }

    #[test]
    fn full_conversation_between_two_passports() {
        let (mut alice, mut bob) = (session(), session());
        alice.save_contact(bob.me.pk, "Bob").unwrap();
        alice.link_changed(true);
        bob.link_changed(true);

        let fx = alice.send(&bob.me.pk, "hi bob").unwrap();
        let pubs = frames_of(&fx, "publish");
        assert_eq!(pubs.len(), 2, "one wrap for Bob, one for Alice's own inbox");
        assert_eq!(pubs[0]["to"], bob.me.pk.to_hex());
        assert_eq!(pubs[1]["to"], alice.me.pk.to_hex());
        assert!(!fx.frames.iter().any(|f| f.contains("hi bob")), "plaintext never crosses the link");

        // Bob receives.
        let rx = bob.handle_frame(&deliver(&pubs[0]["ev"]));
        assert_eq!(rx.arrived.len(), 1);
        assert_eq!(rx.arrived[0].text, "hi bob");
        assert!(!rx.arrived[0].known);
        assert_eq!(bob.inbox.unread_total(), 1);
        // Replays and Alice's self copy are ignored by Bob.
        assert!(bob.handle_frame(&deliver(&pubs[0]["ev"])).arrived.is_empty());
        assert!(!bob.handle_frame(&deliver(&pubs[1]["ev"])).changed, "not p-tagged to Bob");

        // Relay acks Alice's wrap.
        let wrap_id = pubs[0]["ev"]["id"].as_str().unwrap();
        alice.handle_frame(&serde_json::json!({"t":"ok","id":wrap_id,"accepted":["wss://a","wss://b"]}).to_string());
        let chat = alice.inbox.chat(&bob.me.pk.to_hex()).unwrap();
        assert_eq!(chat.messages[0].status, Status::Sent { relays: 2 });
        assert_eq!(alice.inbox.outbox.len(), 1, "self copy still pending");

        // Alice's own copy comes back from a relay: no duplicate.
        let echo = alice.handle_frame(&deliver(&pubs[1]["ev"]));
        assert!(echo.arrived.is_empty());
        assert_eq!(alice.inbox.chat(&bob.me.pk.to_hex()).unwrap().messages.len(), 1);

        // Bob replies; Alice sees it under Bob.
        let reply = frames_of(&bob.send(&alice.me.pk, "hey alice").unwrap(), "publish");
        let rx = alice.handle_frame(&deliver(&reply[0]["ev"]));
        assert_eq!(rx.arrived[0].text, "hey alice");
        assert!(rx.arrived[0].known);
        assert_eq!(alice.inbox.chat(&bob.me.pk.to_hex()).unwrap().messages.len(), 2);
    }

    #[test]
    fn restored_passport_rebuilds_history_from_self_copies() {
        let (mut alice, bob) = (session(), session());
        alice.link_changed(true);
        let pubs = frames_of(&alice.send(&bob.me.pk, "sent before the restore").unwrap(), "publish");
        // Same key, empty inbox (e.g. after a device restore).
        let mut restored = Session::new(Identity::from_secret(alice.me.sk.clone()), vec![], Inbox::default());
        restored.handle_frame(&deliver(&pubs[1]["ev"]));
        let chat = restored.inbox.chat(&bob.me.pk.to_hex()).unwrap();
        assert!(chat.messages[0].out);
        assert_eq!(chat.messages[0].text, "sent before the restore");
    }

    #[test]
    fn offline_messages_queue_and_flush_on_link() {
        let (mut alice, bob) = (session(), session());
        let fx = alice.send(&bob.me.pk, "written offline").unwrap();
        assert!(fx.frames.is_empty());
        assert_eq!(alice.inbox.chat(&bob.me.pk.to_hex()).unwrap().messages[0].status, Status::Queued);
        let up = alice.link_changed(true);
        assert_eq!(frames_of(&up, "hello").len(), 1);
        assert_eq!(frames_of(&up, "publish").len(), 2);
        assert_eq!(alice.inbox.chat(&bob.me.pk.to_hex()).unwrap().messages[0].status, Status::Sending);
        alice.link_changed(false);
        assert_eq!(alice.inbox.chat(&bob.me.pk.to_hex()).unwrap().messages[0].status, Status::Queued);
    }

    #[test]
    fn failed_publish_is_kept_for_retry() {
        let (mut alice, bob) = (session(), session());
        alice.link_changed(true);
        let pubs = frames_of(&alice.send(&bob.me.pk, "x").unwrap(), "publish");
        let id = pubs[0]["ev"]["id"].as_str().unwrap();
        alice.handle_frame(&serde_json::json!({"t":"ok","id":id,"accepted":[],"failed":["wss://a: blocked"]}).to_string());
        assert!(matches!(alice.inbox.chat(&bob.me.pk.to_hex()).unwrap().messages[0].status, Status::Failed { .. }));
        assert_eq!(alice.inbox.outbox.len(), 2);
        let again = alice.link_changed(true);
        assert_eq!(frames_of(&again, "publish").len(), 2);
    }

    #[test]
    fn failure_reason_prefers_a_relays_answer() {
        let bob = session().me.pk;
        let why = |failed: serde_json::Value| {
            let mut alice = session();
            alice.link_changed(true);
            let pubs = frames_of(&alice.send(&bob, "x").unwrap(), "publish");
            let id = pubs[0]["ev"]["id"].as_str().unwrap().to_string();
            alice.handle_frame(&serde_json::json!({"t":"ok","id":id,"accepted":[],"failed":failed}).to_string());
            match &alice.inbox.chat(&bob.to_hex()).unwrap().messages[0].status {
                Status::Failed { why } => why.clone(),
                s => panic!("{s:?}"),
            }
        };
        // 11 default relays: the dead ones answer first, the paid / WoT ones say why.
        assert_eq!(why(serde_json::json!(["wot.nostr.party: unreachable", "127.0.0.1:7: timeout", "eden.nostr.land: blocked: paid relay"])), "blocked");
        assert_eq!(why(serde_json::json!(["a.example: unreachable", "b.example: timeout"])), "unreachable");
        // The host is not read: only the relay's words.
        assert_eq!(why(serde_json::json!(["pow.example: unreachable"])), "unreachable");
        assert_eq!(why(serde_json::json!(["blocked: no host given"])), "blocked");
        assert_eq!(why(serde_json::json!([])), "no relay reachable");
    }

    #[test]
    fn eleven_relays_fit_the_auth_budget_once() {
        // The new default pool: 11 relays. On one link each may ask once; the Passport answers
        // at most AUTH_TOTAL (8) in a minute, so relays 9.. stay unauthenticated until it passes.
        let mut s = session();
        s.link_changed(true);
        let relays: Vec<String> = (0..11).map(|i| format!("wss://r{i}.example")).collect();
        s.handle_frame(&serde_json::json!({"t":"hello","client":"c","relays":relays}).to_string());
        assert_eq!(s.relays.len(), 11);
        let signed = relays
            .iter()
            .filter(|r| !frames_of(&s.handle_frame(&serde_json::json!({"t":"auth","relay":r,"challenge":"c1"}).to_string()), "auth").is_empty())
            .count();
        assert_eq!(signed, AUTH_TOTAL, "documents the cap: 3 of 11 relays get no AUTH within the minute");
    }

    #[test]
    fn junk_and_foreign_frames_are_harmless() {
        let mut s = session();
        let other = session();
        for f in ["", "{}", "[1,2]", r#"{"t":"event","ev":{"kind":1059}}"#, r#"{"t":"ok","id":"zz"}"#] {
            let fx = s.handle_frame(f);
            assert!(fx.arrived.is_empty() && !fx.changed, "{f}");
        }
        // A validly signed wrap p-tagged to us but encrypted to someone else: refused once,
        // then remembered so a replay costs nothing.
        let rumor = crate::nip17::chat_rumor(&other.me.pk, &[s.me.pk], "not yours", 1).unwrap();
        let seal = crate::nip59::seal(&rumor, &other.me.sk, &other.me.pk, 1).unwrap();
        let one_time = SecretKey::generate().unwrap();
        let w = crate::UnsignedEvent {
            pubkey: one_time.public_key(),
            created_at: 1,
            kind: 1059,
            tags: vec![vec!["p".into(), s.me.pk.to_hex()]],
            content: crate::nip44::encrypt(&one_time, &other.me.pk, &seal.to_json().unwrap()).unwrap(),
        }
        .sign(&one_time)
        .unwrap();
        let frame = deliver(&serde_json::to_value(&w).unwrap());
        let fx = s.handle_frame(&frame);
        assert_eq!(fx.refused, 1);
        assert!(fx.arrived.is_empty());
        assert_eq!(s.handle_frame(&frame).refused, 0);
    }

    #[test]
    fn eose_auth_ping_hello() {
        let mut s = session();
        s.link_changed(true);
        assert!(s.handle_frame(r#"{"t":"hello","client":"test","now":1790000000,"relays":["wss://a","http://b"]}"#).status);
        assert_eq!(s.relays, vec!["wss://a".to_string()]);
        let fx = s.handle_frame(r#"{"t":"eose"}"#);
        assert!(s.synced && fx.changed);
        assert_eq!(s.inbox.last_sync, 1_790_000_000);
        assert_eq!(s.since(), 1_790_000_000 - TIME_JITTER - 3600);
        let fx = s.handle_frame(r#"{"t":"auth","relay":"wss://a","challenge":"abc"}"#);
        let a = frames_of(&fx, "auth");
        assert_eq!(a[0]["ev"]["kind"], 22242);
        assert_eq!(frames_of(&s.handle_frame(r#"{"t":"ping"}"#), "pong").len(), 1);
        assert!(s.publish_dm_relays(&["wss://evil".into()]).is_err(), "not the list on screen");
        let fx = s.publish_dm_relays(&["wss://a".into()]).unwrap();
        assert_eq!(frames_of(&fx, "publish")[0]["ev"]["kind"], 10050);
    }

    #[test]
    fn auth_is_answered_sparingly() {
        let mut s = session();
        s.link_changed(true);
        s.handle_frame(r#"{"t":"hello","client":"c","relays":["wss://a","wss://b"]}"#);
        let n = |s: &mut Session, f: &str| frames_of(&s.handle_frame(f), "auth").len();
        assert_eq!(n(&mut s, r#"{"t":"auth","relay":"wss://elsewhere","challenge":"abc"}"#), 0, "not one of our relays");
        assert_eq!(n(&mut s, r#"{"t":"auth","relay":"wss://a","challenge":"sign this please"}"#), 0, "not a nonce");
        assert_eq!(n(&mut s, r#"{"t":"auth","relay":"wss://a","challenge":"c1"}"#), 1);
        assert_eq!(n(&mut s, r#"{"t":"auth","relay":"wss://a","challenge":"c2"}"#), 1, "a reconnect");
        assert_eq!(n(&mut s, r#"{"t":"auth","relay":"wss://a","challenge":"c3"}"#), 0, "rate limited");
        assert_eq!(n(&mut s, r#"{"t":"auth","relay":"wss://b","challenge":"c4"}"#), 1, "per relay");
    }

    #[test]
    fn contacts() {
        let mut s = session();
        let (a, b) = (session().me.pk, session().me.pk);
        s.save_contact(a, " Alice ").unwrap();
        assert!(s.save_contact(b, "alice").is_err(), "names are unique");
        assert!(s.save_contact(b, "Аlicе").is_err(), "look-alike letters (Cyrillic А, е)");
        assert!(s.save_contact(b, "AIice").is_err(), "capital I for l");
        s.save_contact(b, "Bob").unwrap();
        s.save_contact(a, "Alice B").unwrap();
        assert_eq!(s.contacts.len(), 2);
        assert_eq!(s.display_name(&a), "Alice B");
        s.remove_contact(&a);
        assert!(s.display_name(&a).starts_with("npub1"));
        assert!(s.save_contact(a, "").is_err());
    }
}
