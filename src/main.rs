// SPDX-License-Identifier: GPL-3.0-or-later

//! Sanctm: private Nostr messages (NIP-17) written and read on Passport Prime.
//!
//! The key lives here (derived from the KeyOS app seed, or imported); sealing, gift wrapping,
//! unwrapping and verifying happen here. The companion on the phone only moves signed gift
//! wraps between this Passport and Nostr relays. All of that logic is
//! `nip_ping_core::Session`; this file wires it to the UI, sealed storage, haptics and the
//! link to the phone.

mod device;
mod store;
mod theme;
mod transport;

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::time::Duration;

use nip_ping_core::display::{self, ago};
use nip_ping_core::nip19::{chunked_npub, decode_nsec, encode_npub, encode_nsec, parse_pubkey, parse_secret, short_npub};
use nip_ping_core::profile::{self, Profiles, AVATAR_PX};
use nip_ping_core::{derive_from_app_seed, Effects, Identity, Message, Picture, PublicKey, Session, Status};
use slint_keyos_platform::app_ui2;
use slint_keyos_platform::fs::Location;
use slint_keyos_platform::gui_server_api::navigation::filepicker::{AllowedLocations, Location as PickLocation, SelectFileOptions};
use slint_keyos_platform::gui_server_api::navigation::qrscanner::{MatchedQrResult, ScanQrOptions, ScanQrResult};
use slint_keyos_platform::gui_server_api::InputMessage;
use slint_keyos_platform::navigation::{open_qr_scanner, select_file};
use slint_keyos_platform::slint::{
    Color, ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode, VecModel, Weak,
};

app_ui2!("Sanctm");

const SIM: bool = cfg!(not(target_os = "xous"));

const PAGE_LOADING: i32 = 0;
const PAGE_ONBOARDING: i32 = 1;
const PAGE_IMPORT: i32 = 2;
const PAGE_CHATS: i32 = 3;
const PAGE_THREAD: i32 = 4;
const PAGE_NEW_CHAT: i32 = 6;
const PAGE_CONTACT: i32 = 7;
const PAGE_ME: i32 = 8;
const PAGE_ERROR: i32 = 9;
const PAGE_PICTURE: i32 = 10;
const PAGE_ACCOUNTS: i32 = 11;
const PAGE_REMOVE: i32 = 12;

const SCAN_NPUB: i32 = 0;
const SCAN_NSEC: i32 = 1;

/// Pictures held in memory at once (each up to 360 x 480 RGBA, ~690 KB).
const MAX_PICS_IN_MEMORY: usize = 12;
/// Kept pictures read from storage when a chat opens (the newest ones).
const KEPT_PRELOAD: usize = 6;

/// Animations whose frames stay in memory (each up to 20 MB); older ones show their first
/// frame only.
const MAX_ANIMS_IN_MEMORY: usize = 3;

/// A picture in a message, as far as this run knows. `frames`: an animation's (or a video's)
/// frames and how long each shows (ms), empty for a still picture; `file`, `ext`: its own file,
/// for a saved copy that still moves.
enum Pic {
    Loading,
    Ready { w: u32, h: u32, buf: SharedPixelBuffer<Rgba8Pixel>, img: Image, kept: bool, frames: Vec<(Image, u32)>, file: Vec<u8>, ext: &'static str },
    Failed(String),
}

/// The new-chat page: which key "Start chat" would use.
#[derive(Default)]
struct NewChat {
    pk: Option<PublicKey>,
    /// The name@domain address that resolved (or is resolving) to `pk`.
    address: Option<String>,
    /// Lookup sent, no answer yet.
    looking: bool,
    /// What the name field was filled with from their profile (refilled while unchanged).
    prefill: String,
}

struct State {
    store: store::Store,
    seed: [u8; 32],
    /// The accounts on this Passport; `slot` is the one in use.
    accounts: store::Accounts,
    accounts_unreadable: bool,
    slot: u32,
    /// Settings file of the slot in use.
    slot_profile: store::Profile,
    session: Option<Session>,
    link: transport::Link,
    link_up: bool,
    link_conn: u64,
    link_text: String,
    /// Chat on screen (thread or compose).
    open_peer: Option<PublicKey>,
    /// Key on the contact page.
    contact_peer: Option<PublicKey>,
    storage_note: String,
    /// A file that exists but could not be read is never overwritten in this run: a passing
    /// read error must not turn into an empty inbox written over the real one.
    contacts_unreadable: bool,
    inbox_unreadable: bool,
    people_unreadable: bool,
    kept_unreadable: bool,
    /// The companion's last `hello` and `relays` frames on the current link, replayed into a
    /// session started while it stays linked (account switch, first identity).
    companion_hello: Option<String>,
    companion_relays: Option<String>,
    /// Fingerprint of the profiles last written, so avatars are not rewritten on every message.
    people_saved: u64,
    /// Round avatars, by hex key: (picture URL, image).
    avatars: RefCell<HashMap<String, (String, Image)>>,
    pics: HashMap<String, Pic>,
    pic_order: VecDeque<String>,
    kept: Vec<store::Kept>,
    /// Message with its action bar open; message being answered on the compose page.
    selected: Option<String>,
    reply_to: Option<String>,
    /// Message whose picture is on the viewer page, and the frame it shows (animations).
    viewer: Option<String>,
    viewer_frame: usize,
    new_chat: NewChat,
    /// Unsent text per chat (hex key): a draft for one person must never sit, ready to send,
    /// in someone else's composer.
    drafts: HashMap<String, String>,
    /// The chat whose draft is in the composer now.
    draft_peer: Option<String>,
}

thread_local! {
    /// UI-thread handle so link callbacks and timers can reach app state after hopping onto
    /// the event loop.
    static APP: RefCell<Option<(Rc<RefCell<State>>, Weak<AppWindow>)>> = const { RefCell::new(None) };
    /// An npub handed over by the Launcher before an account was ready (the app is still
    /// starting, or on its first-run screen): opened once one is.
    static PENDING_SCAN: RefCell<Option<String>> = const { RefCell::new(None) };
    static SAVE_TIMER: Timer = Timer::default();
    /// When the oldest change not yet written was made (see `schedule_save`).
    static SAVE_PENDING_SINCE: RefCell<Option<std::time::Instant>> = const { RefCell::new(None) };
    static TOAST_TIMER: Timer = Timer::default();
    static CLOCK_TIMER: Timer = Timer::default();
    /// Steps the viewer through an animation's frames.
    static ANIM_TIMER: Timer = Timer::default();
    /// Hides a key shown on the remove page after 30 s.
    static KEY_TIMER: Timer = Timer::default();
}

fn with_app(f: impl FnOnce(&AppWindow, &mut State)) {
    APP.with(|cell| {
        let Some((state, weak)) = cell.borrow().clone() else { return };
        let Some(ui) = weak.upgrade() else { return };
        let Ok(mut st) = state.try_borrow_mut() else {
            log::warn!("state busy; dropped a re-entrant call");
            return;
        };
        f(&ui, &mut st);
    });
}

// ---- formatting ----------------------------------------------------------------------------

/// Avatar tints, picked by the first byte of the key: same person, same colour, everywhere.
const TINTS: [(u8, u8, u8); 8] = [
    (163, 92, 255),
    (224, 86, 155),
    (79, 142, 247),
    (47, 191, 176),
    (242, 153, 74),
    (63, 191, 111),
    (255, 111, 168),
    (111, 108, 255),
];

fn tint(pk: &PublicKey) -> Color {
    let (r, g, b) = TINTS[(pk.as_bytes()[0] % 8) as usize];
    Color::from_rgb_u8(r, g, b)
}

/// A core error as a UI sentence: "name must be 1 to 24 characters" -> "Name must be 1 to 24 characters."
fn sentence(e: impl ToString) -> String {
    let e = e.to_string().replace("link frame: ", "");
    let mut c = e.chars();
    let mut out: String = c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}


/// A failed message can be sent again by hand only while a phone is linked (it also goes out
/// by itself on the next link).
/// Messages from the same side closer than this (seconds) are drawn as one run of bubbles.
const RUN_GAP: u64 = 300;

fn status_meta(now: u64, m: &Message, linked: bool) -> (String, bool) {
    let when = ago(now, m.at);
    if !m.out {
        return (when, false);
    }
    match &m.status {
        Status::Queued => (format!("{when} · queued on Passport"), false),
        Status::Sending => (format!("{when} · sending…"), false),
        Status::Sent { relays } => (format!("{when} · on {relays} relay{}", if *relays == 1 { "" } else { "s" }), false),
        Status::Failed { why } => (format!("{when} · not sent ({why}) · {}", if linked { "tap to resend" } else { "resends when the phone links" }), true),
        Status::Received => (when, false),
    }
}

/// "disappears in 5 min" for a NIP-40 expiration.
/// No addition can overflow: `at` comes from someone else's message and may be near u64::MAX.
fn expires_in(now: u64, at: u64) -> String {
    let d = at.saturating_sub(now);
    let rounded = |unit: u64| (d / unit + u64::from(d % unit >= unit / 2)).max(1);
    match d {
        0..=59 => "disappears in under a minute".into(),
        60..=3299 => format!("disappears in {} min", d.div_ceil(60)),
        3300..=84_599 => format!("disappears in {} h", rounded(3600)),
        _ => format!("disappears in {} d", rounded(86_400)),
    }
}

/// Emoji as compared: without variation selectors and skin tones.
fn bare_emoji(e: &str) -> String {
    e.chars().filter(|c| !matches!(c, '\u{FE0E}' | '\u{FE0F}' | '\u{1F3FB}'..='\u{1F3FF}')).collect()
}

/// Bundled picture for an emoji ("" when there is none: the text is drawn instead). The
/// device fonts have no emoji, so the reactions this app offers are pictures.
fn emoji_icon(e: &str) -> &'static str {
    match bare_emoji(e).as_str() {
        "❤" | "♥" => "heart",
        "👍" => "thumbs",
        "😂" => "joy",
        "🔥" => "fire",
        "🙏" => "pray",
        "👎" => "thumbsdown",
        "★" | "⭐" => "star",
        _ => "",
    }
}

// ---- images --------------------------------------------------------------------------------

/// A profile picture as a round image: the software renderer does not clip to a circle, so
/// the mask is in the pixels (with a one-pixel soft rim).
fn round_avatar(rgba: &[u8], px: u32) -> Option<Image> {
    if rgba.len() != (px * px * 4) as usize {
        return None;
    }
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, px, px);
    let r = px as f32 / 2.0;
    for (i, p) in buf.make_mut_slice().iter_mut().enumerate() {
        let x = (i as u32 % px) as f32 + 0.5 - r;
        let y = (i as u32 / px) as f32 + 0.5 - r;
        let cover = (r - (x * x + y * y).sqrt()).clamp(0.0, 1.0);
        p.a = (p.a as f32 * cover).round() as u8;
    }
    Some(Image::from_rgba8(buf))
}

/// A picture for a bubble, with rounded corners in the pixels.
fn picture_buffer(w: u32, h: u32, rgba: &[u8]) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    if w == 0 || h == 0 || rgba.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba, w, h);
    let rad = 10.0_f32.min(w as f32 / 2.0).min(h as f32 / 2.0);
    let n = rad.ceil() as u32;
    let px = buf.make_mut_slice();
    for cy in 0..n {
        for cx in 0..n {
            let dx = rad - (cx as f32 + 0.5);
            let dy = rad - (cy as f32 + 0.5);
            let cover = (rad - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            if cover >= 1.0 {
                continue;
            }
            for (x, y) in [(cx, cy), (w - 1 - cx, cy), (cx, h - 1 - cy), (w - 1 - cx, h - 1 - cy)] {
                let p = &mut px[(y * w + x) as usize];
                p.a = (p.a as f32 * cover).round() as u8;
            }
        }
    }
    Some(buf)
}

/// The round avatar of `pk`, if profiles are on and its picture has arrived.
fn avatar_of(st: &State, s: &Session, pk: &PublicKey) -> Option<Image> {
    let av = s.profile(pk)?.avatar.as_ref()?;
    cached_avatar(st, &pk.to_hex(), &av.url, &av.rgba)
}

fn cached_avatar(st: &State, hex: &str, url: &str, rgba: &[u8]) -> Option<Image> {
    let mut cache = st.avatars.borrow_mut();
    if let Some((u, img)) = cache.get(hex) {
        if u == url {
            return Some(img.clone());
        }
    }
    let img = round_avatar(rgba, AVATAR_PX)?;
    if cache.len() > 400 {
        cache.clear();
    }
    cache.insert(hex.to_string(), (url.to_string(), img.clone()));
    Some(img)
}

/// How a person is drawn: name, avatar or initial on their tint.
struct Who {
    name: String,
    initial: SharedString,
    tint: Color,
    avatar: Option<Image>,
    /// No contact name, no profile: the name is a short npub.
    mono: bool,
}

/// Montserrat (the device's text font) draws Latin and Cyrillic; a name with no letter it can
/// draw (Arabic, Hebrew, CJK, emoji only) would be an empty row on the device.
fn drawable(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_alphanumeric() || ('\u{00C0}'..='\u{024F}').contains(&c) || ('\u{0400}'..='\u{04FF}').contains(&c))
}

fn who(st: &State, s: &Session, pk: &PublicKey) -> Who {
    let name = s.display_name(pk);
    let blank = !drawable(&name);
    let name = if blank { short_npub(pk) } else { name };
    let mono = blank || (*pk != s.me.pk && s.contact(pk).is_none() && s.profile_label(pk).is_none());
    Who {
        initial: if mono { "?".into() } else { display::initial(&name).into() },
        name,
        tint: tint(pk),
        avatar: avatar_of(st, s, pk),
        mono,
    }
}

/// The line under a name: their NIP-05 address when the phone confirmed it belongs to this key
/// (with a check mark), else the short npub. An address that did not check out is only a claim
/// anyone can make, so it never stands in for the key.
fn id_line(p: Option<&profile::Profile>, pk: &PublicKey) -> (String, bool) {
    match p {
        // Cut at the start if long: the row elides at the end, which would hide the domain.
        Some(p) if p.nip05_ok && !p.nip05.is_empty() => (profile::nip05_short(&p.nip05, profile::NIP05_SHOWN), true),
        _ => (short_npub(pk), false),
    }
}

/// `@name` in a message: ours is "you".
fn mention_name(s: &Session, pk: &PublicKey) -> String {
    if *pk == s.me.pk {
        "you".into()
    } else {
        s.display_name(pk)
    }
}

/// First line of a message for a reply quote.
fn quote_line(s: &Session, m: &Message) -> String {
    let text = display::preview(&display::mentions(&m.text, |pk| mention_name(s, pk)), 80);
    match &m.file {
        Some(f) if text.is_empty() => f.label().to_string(),
        Some(f) => format!("{}: {text}", f.label()),
        None => text,
    }
}

/// Name of an account in the list: its Nostr profile name, else what kind of key it is.
fn account_label(a: &store::Account) -> String {
    if let Some(name) = a.me.as_ref().and_then(|p| p.label()) {
        return name;
    }
    if a.imported {
        "Imported account".into()
    } else {
        format!("Passport account {}", a.index + 1)
    }
}

// ---- refresh -------------------------------------------------------------------------------

fn refresh_status(ui: &AppWindow, st: &State) {
    let app = ui.global::<App>();
    app.set_storage_note(st.storage_note.as_str().into());
    app.set_link_up(st.link_up);
    refresh_accounts(ui, st);
    let Some(s) = st.session.as_ref() else {
        app.set_link_title(if st.link_up { "Phone linked".into() } else { "No phone".into() });
        app.set_link_detail(st.link_text.as_str().into());
        return;
    };
    app.set_my_npub(encode_npub(&s.me.pk).into());
    app.set_my_npub_short(short_npub(&s.me.pk).into());
    app.set_my_npub_chunked(chunked_npub(&s.me.pk).into());
    refresh_me(ui, st, s);
    app.set_synced(s.synced);
    let no_relay = st.link_up && s.relays_total > 0 && s.relays_up == 0;
    let title = match (st.link_up, s.synced) {
        (true, _) if no_relay => "No relays",
        (true, true) => "Phone linked",
        (true, false) => "Syncing",
        (false, _) if SIM => "No phone",
        (false, _) => "Offline",
    };
    app.set_link_title(title.into());
    let detail = if st.link_up {
        let who = if s.companion.is_empty() { "Companion".to_string() } else { s.companion.clone() };
        let relays = if s.relays_total > 0 { format!("{} of {} relays", s.relays_up, s.relays_total) } else { "connecting to relays".into() };
        let sync = if no_relay {
            " · messages wait until one is reachable"
        } else if s.synced {
            " · up to date"
        } else {
            " · fetching gift wraps…"
        };
        format!("{who} · {relays}{sync}")
    } else {
        let queued = s.inbox.outbox.iter().filter(|p| p.primary).count();
        if queued > 0 {
            format!("{} {queued} message{} waiting to go out.", st.link_text, if queued == 1 { "" } else { "s" })
        } else {
            st.link_text.clone()
        }
    };
    app.set_link_detail(detail.into());
    let relays: Vec<SharedString> = s.relays.iter().map(|r| SharedString::from(r.as_str())).collect();
    app.set_relays(ModelRc::new(VecModel::from(relays)));
    let dm: Vec<SharedString> = s.auth_relays.iter().map(|r| SharedString::from(r.as_str())).collect();
    app.set_dm_relays(ModelRc::new(VecModel::from(dm)));
    let when = match (s.dm_relays_at, display::ago(s.now(), s.dm_relays_at)) {
        (0, _) => String::new(),
        (_, a) if a == "now" => "just now".into(),
        (_, a) if a.starts_with(|c: char| c.is_ascii_digit()) => format!("{a} ago"),
        (_, a) => format!("on {a}"),
    };
    app.set_dm_relays_when(when.into());
}

/// The Me page: our own profile, if we have it.
fn refresh_me(ui: &AppWindow, st: &State, s: &Session) {
    let app = ui.global::<App>();
    let label = st.accounts.get(st.slot).map(account_label).unwrap_or_default();
    let p = s.profile(&s.me.pk);
    let name = p.and_then(|p| p.label()).unwrap_or_else(|| label.clone());
    app.set_my_account_label(label.into());
    app.set_my_initial(display::initial(&name).into());
    app.set_my_name(name.into());
    app.set_my_tint(tint(&s.me.pk));
    let av = avatar_of(st, s, &s.me.pk);
    app.set_my_has_avatar(av.is_some());
    app.set_my_avatar(av.unwrap_or_default());
    app.set_my_has_profile(p.is_some());
    app.set_my_handle(p.map(|p| p.handle.clone()).unwrap_or_default().into());
    app.set_my_nip05(p.map(|p| profile::nip05_short(&p.nip05, profile::NIP05_SHOWN)).unwrap_or_default().into());
    app.set_my_nip05_ok(p.is_some_and(|p| p.nip05_ok));
    app.set_my_about(p.map(|p| p.about.clone()).unwrap_or_default().into());
    app.set_my_lud16(p.map(|p| p.lud16.clone()).unwrap_or_default().into());
    app.set_show_profiles(s.show_profiles);
}

fn refresh_accounts(ui: &AppWindow, st: &State) {
    let app = ui.global::<App>();
    let rows: Vec<AccountRow> = st
        .accounts
        .list
        .iter()
        .filter_map(|a| {
            let pk = PublicKey::from_hex(&a.pk).ok()?;
            let label = account_label(a);
            // The account in use shows its live profile; the others their cached copy.
            let live = st.session.as_ref().filter(|s| s.me.pk == pk);
            let avatar = match live {
                Some(s) => avatar_of(st, s, &pk),
                None => a.me.as_ref().and_then(|p| p.avatar.as_ref()).and_then(|av| cached_avatar(st, &a.pk, &av.url, &av.rgba)),
            };
            let (line, verified) = id_line(live.and_then(|s| s.profile(&pk)).or(a.me.as_ref()), &pk);
            Some(AccountRow {
                slot: a.slot as i32,
                initial: display::initial(&label).into(),
                name: label.into(),
                npub: line.into(),
                verified,
                tint: tint(&pk),
                has_avatar: avatar.is_some(),
                avatar: avatar.unwrap_or_default(),
                imported: a.imported,
                current: a.slot == st.slot && st.session.is_some(),
                place: match (a.imported, a.shown_at) {
                    (true, 0) => "In your Passport backup".to_string(),
                    (false, 0) => "In your seed words".to_string(),
                    (true, t) => format!("In your Passport backup · key shown {}", day_label(t)),
                    (false, t) => format!("In your seed words · key shown {}", day_label(t)),
                }
                .into(),
            })
        })
        .collect();
    app.set_accounts(ModelRc::new(VecModel::from(rows)));
    app.set_can_add_account(st.accounts.list.len() < store::MAX_ACCOUNTS);
    app.set_next_account_label(format!("Passport account {}", st.accounts.free_index() + 1).into());
}

/// "Oct 31" (or "Oct 31 2025" in another year), for days kept without the time.
fn day_label(t: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(t);
    // Past a week `ago` gives the date.
    ago(now.max(t) + 7 * 86_400, t)
}

/// Now, from the session's clock (set by the companion when the device clock is not) or the
/// system's.
fn now_secs(st: &State) -> u64 {
    st.session.as_ref().map(|s| s.now()).filter(|t| *t > 1_700_000_000).unwrap_or_else(|| {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
    })
}


fn refresh_chats(ui: &AppWindow, st: &State) {
    let app = ui.global::<App>();
    let Some(s) = st.session.as_ref() else { return };
    let now = s.now();
    let rows: Vec<ChatRow> = s
        .inbox
        .ordered()
        .into_iter()
        .filter_map(|c| {
            let pk = PublicKey::from_hex(&c.peer).ok()?;
            let last = c.last()?;
            let known = s.contact(&pk).is_some() || pk == s.me.pk;
            let w = who(st, s, &pk);
            let body = match &last.file {
                Some(f) if last.text.is_empty() => f.label().to_string(),
                Some(f) => format!("{}: {}", f.label(), last.text),
                None => display::mentions(&last.text, |pk| mention_name(s, pk)),
            };
            Some(ChatRow {
                peer: c.peer.as_str().into(),
                initial: w.initial,
                name: w.name.into(),
                tint: w.tint,
                has_avatar: w.avatar.is_some(),
                avatar: w.avatar.unwrap_or_default(),
                mono: w.mono,
                preview: format!("{}{}", if last.out { "You: " } else { "" }, display::preview(&body, 70)).into(),
                when: ago(now, last.at).into(),
                unread: c.unread as i32,
                known,
            })
        })
        .collect();
    app.set_chats(ModelRc::new(VecModel::from(rows)));
    app.set_unread_total(s.inbox.unread_total() as i32);
    let contacts: Vec<ContactRow> = s
        .contacts
        .iter()
        .map(|c| {
            let av = avatar_of(st, s, &c.pk);
            let (line, verified) = id_line(s.profile(&c.pk), &c.pk);
            ContactRow {
                hex: c.pk.to_hex().into(),
                name: c.name.as_str().into(),
                initial: display::initial(&c.name).into(),
                tint: tint(&c.pk),
                has_avatar: av.is_some(),
                avatar: av.unwrap_or_default(),
                npub: line.into(),
                verified,
            }
        })
        .collect();
    app.set_contacts(ModelRc::new(VecModel::from(contacts)));
}

fn reaction_rows(m: &Message, me: &str) -> Vec<ReactionRow> {
    let mut rows: Vec<(String, ReactionRow)> = Vec::new();
    for r in &m.reactions {
        let key = bare_emoji(&r.emoji);
        match rows.iter_mut().find(|(k, _)| *k == key) {
            Some((_, row)) => {
                row.count += 1;
                row.mine |= r.by == me;
            }
            None => rows.push((
                key,
                ReactionRow { emoji: r.emoji.as_str().into(), icon: emoji_icon(&r.emoji).into(), count: 1, mine: r.by == me },
            )),
        }
    }
    rows.into_iter().map(|(_, r)| r).collect()
}

fn msg_row(st: &State, s: &Session, peer: &PublicKey, m: &Message, now: u64) -> MsgRow {
    let (mut meta, failed) = status_meta(now, m, s.link_up);
    if let Some(e) = m.expires {
        meta.push_str(" · ");
        meta.push_str(&expires_in(now, e));
    }
    let mut text = display::mentions(&m.text, |pk| mention_name(s, pk));
    // The parent is looked up in this chat only: a message from another chat (a group
    // message, say) must not be quoted here under this person's name.
    let chat = s.inbox.chat(&peer.to_hex());
    let (reply, reply_author, reply_text) = match &m.reply_to {
        None => (false, String::new(), String::new()),
        Some(id) => match chat.and_then(|c| c.messages.iter().find(|p| p.id == *id)) {
            Some(p) => (true, if p.out { "You".to_string() } else { s.display_name(peer) }, quote_line(s, p)),
            None => (true, String::new(), "Earlier message".to_string()),
        },
    };
    let mut row = MsgRow {
        id: m.id.as_str().into(),
        out: m.out,
        meta: meta.into(),
        failed,
        sent: m.out && matches!(m.status, Status::Sent { .. }),
        group: m.group,
        reply,
        reply_author: reply_author.into(),
        reply_text: reply_text.into(),
        reactions: ModelRc::new(VecModel::from(reaction_rows(m, &s.me.pk.to_hex()))),
        ..MsgRow::default()
    };
    let kept = st.kept.iter().any(|k| k.msg == m.id);
    let (idle, label, icon) = match &m.file {
        Some(f) if f.is_image() => (Some((1, "Photo", "Tap to open on Passport")), f.label(), "photo"),
        Some(f) if f.label() == "Video" => (Some((7, "Video", "Tap to play it here, without sound")), "Video", "video"),
        Some(f) => {
            let icon = match f.label() {
                "Video" => "video",
                "Voice message" => "voice",
                _ => "file",
            };
            (Some((2, f.label(), "Open it in a phone app")), f.label(), icon)
        }
        None => match (display::picture_url(&m.text), display::video_url(&m.text)) {
            (Some(url), _) => {
                // A message that is only the link shows the card alone.
                if text.trim() == url {
                    text.clear();
                }
                (Some((3, "Picture link", "Tap to load. Your phone fetches it.")), "Picture", "link")
            }
            (None, Some(url)) => {
                if text.trim() == url {
                    text.clear();
                }
                (Some((7, "Video link", "Tap to play it here, without sound")), "Video", "video")
            }
            (None, None) => (None, "", ""),
        },
    };
    if let Some((kind, title, detail)) = idle {
        let (media, title, detail, icon) = match st.pics.get(&m.id) {
            Some(Pic::Ready { w, h, img, kept, frames, ext, .. }) => {
                row.picture = img.clone();
                row.pic_w = *w as i32;
                row.pic_h = *h as i32;
                row.kept = *kept;
                row.anim = match (frames.is_empty(), *ext) {
                    (true, _) => "",
                    (false, "mp4") => "Video",
                    (false, _) => "GIF",
                }
                .into();
                (5, String::new(), String::new(), icon)
            }
            Some(Pic::Loading) if kind == 3 => (4, "Loading picture…".into(), "Your phone fetches it".into(), icon),
            Some(Pic::Loading) if kind == 7 => (4, "Getting the video ready…".into(), "Your phone fetches it, the Passport plays it".into(), icon),
            Some(Pic::Loading) => (4, "Opening on Passport…".into(), "Your phone fetches the sealed file".into(), icon),
            Some(Pic::Failed(e)) => (6, format!("{label} did not open"), format!("{} Tap to try again.", sentence(e)), "retry"),
            None if kept => (kind, "Kept in Sanctm".into(), "Tap to show".into(), "photo"),
            None => (kind, title.to_string(), detail.to_string(), icon),
        };
        row.media = media;
        row.media_title = title.into();
        row.media_detail = detail.into();
        row.media_icon = icon.into();
    }
    row.text = text.into();
    row
}

fn refresh_thread(ui: &AppWindow, st: &State) {
    let (Some(s), Some(peer)) = (st.session.as_ref(), st.open_peer) else { return };
    let app = ui.global::<App>();
    let now = s.now();
    let w = who(st, s, &peer);
    app.set_thread_peer(peer.to_hex().into());
    let (line, verified) = id_line(s.profile(&peer), &peer);
    // An unnamed stranger's npub is already the title; a verified address still helps.
    app.set_thread_subtitle(if w.mono && !verified { SharedString::new() } else { line.into() });
    app.set_thread_subtitle_verified(verified);
    app.set_thread_name(w.name.into());
    app.set_thread_mono(w.mono);
    app.set_thread_initial(w.initial);
    app.set_thread_tint(w.tint);
    app.set_thread_has_avatar(w.avatar.is_some());
    app.set_thread_avatar(w.avatar.unwrap_or_default());
    app.set_thread_known(s.contact(&peer).is_some() || peer == s.me.pk);
    let rows: Vec<MsgRow> = s
        .inbox
        .chat(&peer.to_hex())
        .map(|c| {
            let mut prev: Option<&Message> = None;
            c.messages
                .iter()
                .map(|m| {
                    let mut row = msg_row(st, s, &peer, m, now);
                    // Same side, same kind, under 5 minutes apart: one run of bubbles.
                    row.run_start = !prev.is_some_and(|p| p.out == m.out && p.group == m.group && m.at.saturating_sub(p.at) < RUN_GAP);
                    prev = Some(m);
                    row
                })
                .collect()
        })
        .unwrap_or_default();
    app.set_messages(ModelRc::new(VecModel::from(rows)));
    refresh_selection(ui, st);
    let replying = st.reply_to.as_ref().and_then(|id| s.inbox.chat(&peer.to_hex()).and_then(|c| c.messages.iter().find(|m| m.id == *id)));
    app.set_replying(replying.is_some());
    if let Some(p) = replying {
        app.set_reply_author(if p.out { "yourself".into() } else { s.display_name(&peer).into() });
        app.set_reply_text(quote_line(s, p).into());
    }
}

fn refresh_selection(ui: &AppWindow, st: &State) {
    let app = ui.global::<App>();
    let m = st.selected.as_ref().and_then(|id| st.session.as_ref().and_then(|s| s.inbox.find_message(id).map(|m| (s, m))));
    let Some((s, m)) = m else {
        app.set_selected_id(SharedString::new());
        app.set_selected_emoji(SharedString::new());
        app.set_selected_can_keep(false);
        app.set_selected_failed(false);
        return;
    };
    let me = s.me.pk.to_hex();
    app.set_selected_id(m.id.as_str().into());
    app.set_selected_emoji(m.reactions.iter().find(|r| r.by == me).map(|r| emoji_icon(&r.emoji)).unwrap_or_default().into());
    app.set_selected_can_keep(matches!(st.pics.get(&m.id), Some(Pic::Ready { kept: false, .. })));
    app.set_selected_failed(m.out && matches!(m.status, Status::Failed { .. }));
}

fn refresh_contact(ui: &AppWindow, st: &State) {
    let (Some(s), Some(peer)) = (st.session.as_ref(), st.contact_peer) else { return };
    let app = ui.global::<App>();
    let saved = s.contact(&peer).map(|c| c.name.clone());
    let w = who(st, s, &peer);
    let p = s.profile(&peer);
    app.set_contact_hex(peer.to_hex().into());
    app.set_contact_title(match (&saved, p.and_then(|p| p.label())) {
        (Some(n), _) => n.as_str().into(),
        (None, Some(l)) => l.into(),
        (None, None) => "Unknown sender".into(),
    });
    app.set_contact_saved(saved.is_some());
    app.set_contact_npub(encode_npub(&peer).into());
    app.set_contact_npub_short(short_npub(&peer).into());
    app.set_contact_npub_chunked(chunked_npub(&peer).into());
    app.set_contact_initial(w.initial);
    app.set_contact_tint(w.tint);
    app.set_contact_has_avatar(w.avatar.is_some());
    app.set_contact_avatar(w.avatar.unwrap_or_default());
    app.set_contact_has_profile(p.is_some());
    app.set_contact_profile_name(p.map(|p| p.name.clone()).unwrap_or_default().into());
    app.set_contact_handle(p.map(|p| p.handle.clone()).unwrap_or_default().into());
    app.set_contact_nip05(p.map(|p| profile::nip05_short(&p.nip05, profile::NIP05_SHOWN)).unwrap_or_default().into());
    app.set_contact_nip05_ok(p.is_some_and(|p| p.nip05_ok));
    app.set_contact_about(p.map(|p| p.about.clone()).unwrap_or_default().into());
    app.set_contact_lud16(p.map(|p| p.lud16.clone()).unwrap_or_default().into());
}

/// The new-chat page's result card: who the key in the field is.
fn refresh_new_chat(ui: &AppWindow, st: &mut State) {
    let app = ui.global::<App>();
    app.set_new_looking(st.new_chat.looking);
    let (Some(s), Some(pk)) = (st.session.as_ref(), st.new_chat.pk) else {
        app.set_new_found(false);
        return;
    };
    let p = s.profile(&pk);
    let av = avatar_of(st, s, &pk);
    let contact = s.contact(&pk).map(|c| c.name.clone());
    let pname = p.and_then(|p| p.label()).unwrap_or_default();
    let name = contact.clone().unwrap_or_else(|| pname.clone());
    app.set_new_found(true);
    app.set_new_found_name(name.as_str().into());
    app.set_new_found_npub(short_npub(&pk).into());
    app.set_new_found_initial(if name.is_empty() { "?".into() } else { display::initial(&name).into() });
    app.set_new_found_tint(tint(&pk));
    app.set_new_found_has_avatar(av.is_some());
    app.set_new_found_avatar(av.unwrap_or_default());
    app.set_new_found_contact(contact.clone().unwrap_or_default().into());
    // A looked-up address was resolved by the phone just now; a profile's own NIP-05 is shown
    // as verified only if the phone checked it.
    let (nip05, verified) = match (&st.new_chat.address, p) {
        (Some(a), _) => (profile::nip05_short(a, profile::NIP05_SHOWN), true),
        (None, Some(p)) if !p.nip05.is_empty() => (profile::nip05_short(&p.nip05, profile::NIP05_SHOWN), p.nip05_ok),
        _ => (String::new(), false),
    };
    app.set_new_found_nip05(nip05.into());
    app.set_new_found_verified(verified);
    // Their profile name fills the name field, unless the user typed one.
    if contact.is_none() && !pname.is_empty() {
        let current = app.get_new_name().to_string();
        if current.is_empty() || current == st.new_chat.prefill {
            let fill: String = pname.chars().take(nip_ping_core::session::MAX_NAME_CHARS).collect::<String>().trim().to_string();
            st.new_chat.prefill = fill.clone();
            app.set_new_name(fill.into());
        }
    }
}

fn refresh_viewer(ui: &AppWindow, st: &State) {
    let app = ui.global::<App>();
    let Some(id) = st.viewer.as_ref() else { return };
    let Some(Pic::Ready { w, h, img, kept, frames, ext, .. }) = st.pics.get(id) else { return };
    app.set_viewer_id(id.as_str().into());
    match frames.get(st.viewer_frame % frames.len().max(1)) {
        Some((frame, ms)) => {
            app.set_viewer_picture(frame.clone());
            let ms = *ms;
            if !ANIM_TIMER.with(|t| t.running()) {
                ANIM_TIMER.with(|t| t.start(TimerMode::SingleShot, Duration::from_millis(u64::from(ms)), next_frame));
            }
        }
        None => app.set_viewer_picture(img.clone()),
    }
    app.set_viewer_w(*w as i32);
    app.set_viewer_h(*h as i32);
    app.set_viewer_kept(*kept);
    app.set_viewer_moving(!frames.is_empty());
    app.set_viewer_video(*ext == "mp4");
    app.set_last_save(st.slot_profile.last_save.as_ref().map(save_label).unwrap_or_default().into());
    let title = match (st.session.as_ref(), st.open_peer) {
        (Some(s), Some(peer)) => match s.inbox.find_message(id) {
            Some(m) if m.out => "Picture you sent".to_string(),
            _ => format!("From {}", s.display_name(&peer)),
        },
        _ => "Picture".into(),
    };
    app.set_viewer_title(title.into());
}

fn refresh_all(ui: &AppWindow, st: &mut State) {
    refresh_status(ui, st);
    refresh_chats(ui, st);
    refresh_thread(ui, st);
    refresh_contact(ui, st);
    refresh_new_chat(ui, st);
}

fn toast(ui: &AppWindow, text: &str) {
    ui.global::<App>().set_toast(text.into());
    TOAST_TIMER.with(|t| {
        t.start(TimerMode::SingleShot, Duration::from_secs(6), || {
            with_app(|ui, _| ui.global::<App>().set_toast(SharedString::new()));
        })
    });
}

fn show_error(ui: &AppWindow, title: &str, body: &str, soft: bool) {
    let app = ui.global::<App>();
    app.set_error_title(title.into());
    app.set_error_body(body.into());
    app.set_error_soft(soft);
    app.set_error_retry(false);
    app.set_page(PAGE_ERROR);
    if !soft {
        device::feedback_err();
    }
}

// ---- persistence ---------------------------------------------------------------------------

/// Storage tag of an identity: its contacts and chats files are named after it. Keyed by
/// the app seed, so the file names do not give away the npub.
fn data_id(seed: &[u8; 32], pk: &PublicKey) -> String { nip_ping_core::vault::file_tag(seed, &pk.to_hex()) }

/// Changes when a profile or avatar is added or replaced; avatars are big, so the people
/// file is only rewritten then.
fn people_fingerprint(p: &Profiles) -> u64 {
    let mut h = DefaultHasher::new();
    for (k, v) in &p.by_key {
        (k, v.created_at, v.nip05_ok, v.avatar.as_ref().map(|a| a.url.as_str())).hash(&mut h);
    }
    h.finish()
}

fn save_now(st: &mut State) {
    let Some(s) = st.session.as_ref() else { return };
    let id = data_id(&st.seed, &s.me.pk);
    let mut r = Ok(());
    if !st.inbox_unreadable {
        r = st.store.save_inbox(&id, &s.inbox);
    }
    if !st.contacts_unreadable {
        r = r.and_then(|_| st.store.save_contacts(&id, &s.contacts));
    }
    let fp = people_fingerprint(&s.profiles);
    if !st.people_unreadable && fp != st.people_saved {
        r = r.and_then(|_| st.store.save_people(&id, &s.profiles));
        if r.is_ok() {
            st.people_saved = fp;
        }
    }
    // Kept pictures of messages that are gone (deleted, expired, chat removed) go too; not
    // while the inbox could not be read: "gone" would then mean every kept picture.
    let gone: Vec<String> = st.kept.iter().filter(|k| s.inbox.find_message(&k.msg).is_none()).map(|k| k.msg.clone()).collect();
    if !gone.is_empty() && !st.kept_unreadable && !st.inbox_unreadable {
        for m in &gone {
            if let Err(e) = st.store.remove_picture(&id, m) {
                log::warn!("storage: {e}");
            }
            st.pics.remove(m);
        }
        st.kept.retain(|k| !gone.contains(&k.msg));
        r = r.and_then(|_| st.store.save_kept(&id, &st.kept));
    }
    // The DM relay list the session published by itself (or the user did), and when.
    if s.auth_relays != st.slot_profile.dm_relays || s.dm_relays_at != st.slot_profile.dm_relays_at {
        st.slot_profile.dm_relays = s.auth_relays.clone();
        st.slot_profile.dm_relays_at = s.dm_relays_at;
        if let Err(e) = st.store.save_profile(st.slot, &st.slot_profile) {
            log::warn!("storage: {e}");
        }
    }
    // The account list shows each account's own profile; keep its copy current.
    let me = if s.show_profiles { s.profiles.get(&s.me.pk).cloned() } else { None };
    let slot = st.slot;
    if let Some(a) = st.accounts.get_mut(slot) {
        let stale = match (&a.me, &me) {
            (None, None) => false,
            (Some(x), Some(y)) => x.created_at != y.created_at || x.avatar.as_ref().map(|v| &v.url) != y.avatar.as_ref().map(|v| &v.url),
            _ => true,
        };
        if stale && !st.accounts_unreadable {
            a.me = me;
            if let Err(e) = st.store.save_accounts(&st.accounts) {
                log::warn!("storage: {e}");
            }
        }
    }
    match r {
        Ok(()) if st.inbox_unreadable || st.contacts_unreadable || st.people_unreadable => {}
        Ok(()) => st.storage_note.clear(),
        Err(e) => {
            log::error!("storage: {e}");
            st.storage_note = format!("Could not save: {e}");
        }
    }
}

/// Bursts (an initial sync can bring hundreds of wraps) are written once, 700 ms after
/// the last change, but never later than 3 s after the first unsaved one: a phone that keeps
/// sending (one wrap every 500 ms is enough) must not keep chats, contacts and what the user
/// wrote off storage until the next crash or power loss.
fn schedule_save() {
    let first = SAVE_PENDING_SINCE.with(|c| *c.borrow_mut().get_or_insert_with(std::time::Instant::now));
    if first.elapsed() >= Duration::from_secs(3) && SAVE_TIMER.with(|t| t.running()) {
        return; // due: let the pending save fire instead of pushing it back again
    }
    SAVE_TIMER.with(|t| {
        t.start(TimerMode::SingleShot, Duration::from_millis(700), || {
            SAVE_PENDING_SINCE.with(|c| *c.borrow_mut() = None);
            with_app(|ui, st| {
                save_now(st);
                refresh_status(ui, st);
            })
        })
    });
}

// ---- session plumbing ----------------------------------------------------------------------

fn on_thread(ui: &AppWindow) -> bool { matches!(ui.global::<App>().get_page(), PAGE_THREAD | PAGE_PICTURE) }

/// Put the cursor in the thread's message field.
fn focus_composer(ui: &AppWindow) {
    let app = ui.global::<App>();
    app.set_focus_tick(app.get_focus_tick().wrapping_add(1));
}

fn apply(ui: &AppWindow, st: &mut State, fx: Effects) {
    let Effects { frames, changed, status, arrived, refused, nip05, pictures, videos, .. } = fx;
    for f in &frames {
        st.link.send(f);
    }
    // Videos are decoded away from the UI thread (seconds on the device), then shown like an
    // animated picture.
    for v in videos {
        std::thread::spawn(move || {
            let r = nip_ping_core::video::decode_video(&v.file);
            let pic = Picture::shown(v.msg, r, v.file, "mp4");
            let _ = slint_keyos_platform::slint::invoke_from_event_loop(move || {
                with_app(|ui, st| {
                    picture_arrived(st, pic);
                    refresh_thread(ui, st);
                    refresh_viewer(ui, st);
                })
            });
        });
    }
    if refused > 0 {
        log::warn!("{refused} gift wrap(s) addressed to us did not open");
    }
    let mut changed = changed;
    if let (Some(s), Some(peer)) = (st.session.as_mut(), st.open_peer) {
        if on_thread(ui) {
            changed |= s.inbox.mark_read(&peer.to_hex());
        }
    }
    if let Some((q, r)) = nip05 {
        nip05_answer(ui, st, &q, r);
    }
    let pics = !pictures.is_empty();
    for p in pictures {
        picture_arrived(st, p);
    }
    if changed || pics {
        if changed {
            prune_pictures(ui, st);
            schedule_save();
            refresh_chats(ui, st);
            refresh_contact(ui, st);
            refresh_new_chat(ui, st);
        }
        refresh_thread(ui, st);
        refresh_viewer(ui, st);
    }
    if status || changed {
        refresh_status(ui, st);
    }
    if let Some(a) = arrived.last() {
        device::feedback_message();
        let in_view = on_thread(ui) && st.open_peer == Some(a.peer);
        if !in_view {
            // Only a contact is announced by name: a stranger's profile could call itself
            // "Note to self" or borrow a contact's name.
            let who = match st.session.as_ref() {
                Some(s) if a.known => s.display_name(&a.peer),
                _ => format!("{} (not a contact)", short_npub(&a.peer)),
            };
            let n = arrived.len();
            let msg = if n == 1 { format!("New message from {who}") } else { format!("{n} new messages") };
            toast(ui, &msg);
        }
    }
}

fn set_link(ui: &AppWindow, st: &mut State, status: transport::Status) {
    st.link_text = status.text;
    let fresh = status.up && status.conn != st.link_conn;
    let changed = status.up != st.link_up || fresh;
    st.link_up = status.up;
    if status.up {
        st.link_conn = status.conn;
    }
    if fresh || !status.up {
        st.companion_hello = None;
        st.companion_relays = None;
        // Fetches in flight die with the link (the session forgets them): back to the card.
        st.pics.retain(|_, p| !matches!(p, Pic::Loading));
        st.new_chat.looking = false;
    }
    if changed {
        if let Some(s) = st.session.as_mut() {
            let fx = s.link_changed(status.up);
            apply(ui, st, fx);
        }
        refresh_thread(ui, st);
        refresh_new_chat(ui, st);
    }
    refresh_status(ui, st);
}

// ---- accounts ------------------------------------------------------------------------------

/// The key a slot uses.
fn identity_of(seed: &[u8; 32], slot: u32, p: &store::Profile) -> Result<Identity, String> {
    match p.nsec.as_deref() {
        Some(nsec) => decode_nsec(nsec).map(Identity::from_secret).map_err(|e| format!("Imported key unreadable: {e}")),
        None => derive_from_app_seed(seed, p.index.unwrap_or(slot)).map_err(|e| format!("Key derivation failed: {e}")),
    }
}

/// The account list, built on first run from the slots Sanctm used before it had one
/// (`profile-0.json`, and `profile-1.json` from the simulator's second persona).
fn read_accounts(st: &mut State) -> Result<store::Accounts, String> {
    if let Some(a) = st.store.load_accounts()? {
        return Ok(a);
    }
    let a = accounts_from_slots(st, false)?;
    if !a.list.is_empty() {
        st.store.save_accounts(&a)?;
    }
    Ok(a)
}

/// Accounts found in `profile-<slot>.json` files. `lenient`: skip unreadable ones (used to
/// rebuild a damaged account list) instead of stopping.
fn accounts_from_slots(st: &State, lenient: bool) -> Result<store::Accounts, String> {
    let mut a = store::Accounts::default();
    for slot in 0..store::SLOTS {
        let p = match st.store.load_profile(slot) {
            Ok(Some(p)) if !p.removed => p,
            Ok(_) => continue,
            Err(e) if lenient => {
                log::warn!("storage: {e}");
                continue;
            }
            Err(e) => return Err(e),
        };
        let id = match identity_of(&st.seed, slot, &p) {
            Ok(id) => id,
            Err(e) if lenient => {
                log::warn!("{e}");
                continue;
            }
            Err(e) => return Err(e),
        };
        if a.list.iter().any(|x| x.pk == id.pk.to_hex()) {
            continue;
        }
        a.list.push(store::Account { slot, pk: id.pk.to_hex(), imported: p.nsec.is_some(), index: p.index.unwrap_or(slot), me: None, shown_at: 0 });
    }
    a.current = a.list.first().map(|x| x.slot).unwrap_or(0);
    Ok(a)
}

/// Read the account list and open the account in use (or the first-run screen).
fn start(ui: &AppWindow, st: &mut State) {
    match read_accounts(st) {
        Ok(a) => {
            st.accounts = a;
            st.accounts_unreadable = false;
        }
        Err(e) => {
            st.accounts_unreadable = true;
            st.session = None;
            refresh_status(ui, st);
            show_error(ui, "Storage problem", &format!("This Passport could not read the Sanctm account list: {e}"), false);
            ui.global::<App>().set_error_retry(true);
            return;
        }
    }
    let current = st.accounts.current;
    load_account(ui, st, current, None);
}

fn reset_account_view(ui: &AppWindow, st: &mut State) {
    st.session = None;
    st.open_peer = None;
    st.contact_peer = None;
    st.selected = None;
    st.reply_to = None;
    st.viewer = None;
    st.viewer_frame = 0;
    ANIM_TIMER.with(|t| t.stop());
    st.pics.clear();
    st.pic_order.clear();
    st.kept.clear();
    st.new_chat = NewChat::default();
    st.drafts.clear();
    st.draft_peer = None;
    st.storage_note.clear();
    let app = ui.global::<App>();
    app.set_compose_text(SharedString::new());
    app.set_toast(SharedString::new());
    ui.global::<Confirm>().set_action(SharedString::new());
}

/// Load (or start onboarding for) the account in `slot`. `ready`: its identity, when the
/// caller already has it (a key derivation is not free on the device).
fn load_account(ui: &AppWindow, st: &mut State, slot: u32, ready: Option<Identity>) {
    reset_account_view(ui, st);
    let app = ui.global::<App>();
    let Some(acct) = st.accounts.get(slot).cloned() else {
        st.slot = slot;
        app.set_has_identity(false);
        refresh_status(ui, st);
        app.set_page(PAGE_ONBOARDING);
        return;
    };
    st.slot = slot;
    let profile = match st.store.load_profile(slot) {
        Ok(Some(p)) => p,
        // The list knows the slot: a derived key needs nothing more; an imported one does.
        Ok(None) if !acct.imported => store::Profile { index: Some(acct.index), ..Default::default() },
        Ok(None) => {
            refresh_status(ui, st);
            show_error(ui, "Imported key missing", "The sealed file holding this account's key is gone from this Passport.", false);
            app.set_error_retry(true);
            return;
        }
        Err(e) => {
            refresh_status(ui, st);
            show_error(ui, "Storage problem", &format!("This Passport could not read the Sanctm profile: {e}"), false);
            app.set_error_retry(true);
            return;
        }
    };
    let identity = match ready {
        Some(id) => id,
        None => match identity_of(&st.seed, slot, &profile) {
            Ok(id) => id,
            Err(e) => {
                refresh_status(ui, st);
                show_error(ui, "Account unreadable", &e, false);
                app.set_error_retry(true);
                return;
            }
        },
    };
    app.set_imported(profile.nsec.is_some());
    app.set_has_identity(true);
    let id = data_id(&st.seed, &identity.pk);
    st.contacts_unreadable = false;
    st.inbox_unreadable = false;
    st.people_unreadable = false;
    st.kept_unreadable = false;
    let contacts = st.store.load_contacts(&id).unwrap_or_else(|e| {
        log::error!("storage: {e}");
        st.contacts_unreadable = true;
        st.storage_note = "Saved contacts could not be read. They are left untouched; new ones are not saved until Sanctm restarts.".into();
        Vec::new()
    });
    let inbox = st.store.load_inbox(&id).unwrap_or_else(|e| {
        log::error!("storage: {e}");
        st.inbox_unreadable = true;
        st.storage_note = "Saved chats could not be read. They are left untouched; new messages are not saved until Sanctm restarts.".into();
        Default::default()
    });
    // Profiles are a cache: unreadable means fetched again, but the file is left alone.
    let people = st.store.load_people(&id).unwrap_or_else(|e| {
        log::error!("storage: {e}");
        st.people_unreadable = true;
        Profiles::default()
    });
    st.people_saved = people_fingerprint(&people);
    st.kept = st.store.load_kept(&id).unwrap_or_else(|e| {
        log::error!("storage: {e}");
        st.kept_unreadable = true;
        Vec::new()
    });
    log::info!("account slot {slot}: {}", short_npub(&identity.pk));
    // Keep the list's cached key in step with the slot file.
    let pk_hex = identity.pk.to_hex();
    let mut list_changed = st.accounts.current != slot;
    if let Some(a) = st.accounts.get_mut(slot) {
        if a.pk != pk_hex {
            a.pk = pk_hex;
            a.me = None;
            list_changed = true;
        }
    }
    st.accounts.current = slot;
    if list_changed && !st.accounts_unreadable {
        if let Err(e) = st.store.save_accounts(&st.accounts) {
            log::warn!("storage: {e}");
        }
    }
    let identity_pk_hex = identity.pk.to_hex();
    let mut session = Session::new(identity, contacts, inbox);
    session.profiles = people;
    session.show_profiles = profile.show_profiles.unwrap_or(true);
    session.auth_relays = profile.dm_relays.clone();
    session.dm_relays_at = profile.dm_relays_at;
    session.wiped_at = st.accounts.wiped_at(&st.store.wipe_tag(&identity_pk_hex));
    st.slot_profile = profile;
    st.session = Some(session);
    if st.link_up {
        if let Some(s) = st.session.as_mut() {
            // The companion introduced itself (name, relays, clock) to the previous session.
            for f in [&st.companion_hello, &st.companion_relays].into_iter().flatten() {
                let _ = s.handle_frame(f);
            }
            let fx = s.link_changed(true);
            apply(ui, st, fx);
        }
    }
    refresh_all(ui, st);
    app.set_page(PAGE_CHATS);
    take_pending_scan(ui, st);
}

/// Add an account (a new derived key, or an imported nsec) and switch to it.
fn add_account(ui: &AppWindow, st: &mut State, nsec: Option<String>) -> Result<(), String> {
    if st.accounts_unreadable {
        // "Start over" after a damaged list: rebuild it from the slot files, never drop keys.
        st.accounts = accounts_from_slots(st, true)?;
        st.accounts_unreadable = false;
    }
    if st.accounts.list.len() >= store::MAX_ACCOUNTS {
        return Err(format!("This Passport holds {} accounts. Remove one first.", store::MAX_ACCOUNTS));
    }
    // A free slot has no file at all (or a forgotten one): a file the lenient rebuild skipped
    // because it could not be read may hold an imported key, and must never be written over.
    let free = |s: u32| st.accounts.get(s).is_none() && matches!(st.store.load_profile(s), Ok(None) | Ok(Some(store::Profile { removed: true, .. })));
    let slot = (0..store::SLOTS).find(|s| free(*s)).ok_or("No free account slot on this Passport.")?;
    let imported = nsec.is_some();
    let mut index = if imported { 0 } else { st.accounts.free_index() };
    let mut profile = store::Profile { nsec, index: (!imported).then_some(index), ..Default::default() };
    let mut identity = identity_of(&st.seed, slot, &profile)?;
    if imported && st.accounts.list.iter().any(|a| a.pk == identity.pk.to_hex()) {
        return Err("That account is already on this Passport.".into()); // list was rebuilt above
    }
    // A derived key that is already here (imported earlier as an nsec): take the next index.
    while !imported && st.accounts.list.iter().any(|a| a.pk == identity.pk.to_hex()) && index < 1000 {
        index += 1;
        profile.index = Some(index);
        identity = identity_of(&st.seed, slot, &profile)?;
    }
    save_now(st);
    st.store.save_profile(slot, &profile)?;
    st.accounts.list.push(store::Account { slot, pk: identity.pk.to_hex(), imported, index, me: None, shown_at: 0 });
    let before = st.accounts.current;
    st.accounts.current = slot;
    if let Err(e) = st.store.save_accounts(&st.accounts) {
        st.accounts.list.retain(|a| a.slot != slot);
        st.accounts.current = before;
        let _ = st.store.forget_profile(slot);
        return Err(e);
    }
    load_account(ui, st, slot, Some(identity));
    Ok(())
}

fn switch_account(ui: &AppWindow, st: &mut State, slot: u32) {
    if st.accounts.get(slot).is_none() {
        return;
    }
    if slot == st.slot && st.session.is_some() {
        ui.global::<App>().set_page(PAGE_CHATS);
        return;
    }
    save_now(st);
    load_account(ui, st, slot, None);
    if st.session.is_some() {
        let label = st.accounts.get(slot).map(account_label).unwrap_or_default();
        toast(ui, &format!("Now using {label}."));
    }
}

/// Groups of four characters, for reading a key off the screen.
fn chunked(s: &str) -> String { s.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ") }

/// The remove page for an account that is not in use.
fn start_remove(ui: &AppWindow, st: &mut State, slot: u32) {
    let Some(acct) = st.accounts.get(slot).cloned() else { return };
    if slot == st.slot && st.session.is_some() {
        return;
    }
    let app = ui.global::<App>();
    app.set_remove_slot(slot as i32);
    app.set_remove_name(account_label(&acct).into());
    app.set_remove_imported(acct.imported);
    app.set_remove_nsec(SharedString::new());
    app.set_remove_nsec_chunked(SharedString::new());
    app.set_remove_shown(if acct.shown_at == 0 { SharedString::new() } else { format!("Key shown {}.", day_label(acct.shown_at)).into() });
    app.set_page(PAGE_REMOVE);
}

/// Put the account's nsec on the remove page, so it can be written down first.
fn show_remove_key(ui: &AppWindow, st: &mut State) {
    let app = ui.global::<App>();
    let slot = app.get_remove_slot().max(0) as u32;
    let key = st.store.load_profile(slot).map_err(|e| e.to_string()).and_then(|p| {
        let p = p.or_else(|| st.accounts.get(slot).filter(|a| !a.imported).map(|a| store::Profile { index: Some(a.index), ..Default::default() }));
        p.ok_or_else(|| "its key file is gone".to_string()).and_then(|p| identity_of(&st.seed, slot, &p))
    });
    match key {
        Ok(id) => {
            let nsec = encode_nsec(&id.sk);
            app.set_remove_nsec_chunked(chunked(&nsec).into());
            app.set_remove_nsec(nsec.into());
            // Kept by day only: when, not how often.
            let day = now_secs(st) / 86_400 * 86_400;
            if let Some(a) = st.accounts.get_mut(slot) {
                a.shown_at = day;
            }
            if let Err(e) = st.store.save_accounts(&st.accounts) {
                log::warn!("storage: {e}");
            }
            app.set_remove_shown(format!("Key shown {}.", day_label(day)).into());
            KEY_TIMER.with(|t| {
                t.start(TimerMode::SingleShot, Duration::from_secs(30), || {
                    with_app(|ui, _| {
                        let app = ui.global::<App>();
                        app.set_remove_nsec(SharedString::new());
                        app.set_remove_nsec_chunked(SharedString::new());
                    })
                })
            });
        }
        Err(e) => toast(ui, &format!("The key cannot be shown: {}", sentence(e))),
    }
}

/// Remove an account that is not in use, for good: its key file, chats, contacts, profiles and
/// kept pictures are deleted, and the account list remembers when, so the same key added again
/// shows nothing older (relays still hold the old gift wraps). A derived account's index is
/// retired: a new Passport key is a different key.
fn remove_account(ui: &AppWindow, st: &mut State, slot: u32) {
    if slot == st.slot && st.session.is_some() {
        return;
    }
    let Some(acct) = st.accounts.get(slot).cloned() else { return };
    // Kept to the day, rounded up to the next UTC midnight: nothing from before the removal
    // comes back (a same-day re-add also hides that day's new messages).
    let now = (now_secs(st) / 86_400 + 1) * 86_400;
    let before = st.accounts.clone();
    st.accounts.list.retain(|a| a.slot != slot);
    let tag = st.store.wipe_tag(&acct.pk);
    st.accounts.wiped.retain(|(k, _)| *k != tag);
    st.accounts.wiped.push((tag, now));
    if !acct.imported && !st.accounts.retired.contains(&acct.index) {
        st.accounts.retired.push(acct.index);
    }
    if let Err(e) = st.store.save_accounts(&st.accounts) {
        st.accounts = before;
        toast(ui, &format!("Not removed: {e}"));
        return;
    }
    let mut problem = st.store.forget_profile(slot).err();
    if let Ok(pk) = PublicKey::from_hex(&acct.pk) {
        if let Err(e) = st.store.forget_identity(&data_id(&st.seed, &pk)) {
            problem.get_or_insert(e);
        }
    }
    let app = ui.global::<App>();
    KEY_TIMER.with(|t| t.stop());
    app.set_remove_nsec(SharedString::new());
    app.set_remove_nsec_chunked(SharedString::new());
    refresh_accounts(ui, st);
    app.set_page(PAGE_ACCOUNTS);
    match problem {
        Some(e) => {
            log::error!("storage: {e}");
            toast(ui, "Account removed. A file could not be deleted; it stays sealed and is never shown again.");
        }
        None => toast(ui, "Account removed from this Passport for good."),
    }
}

fn set_show_profiles(ui: &AppWindow, st: &mut State, on: bool) {
    let Some(s) = st.session.as_mut() else { return };
    if s.show_profiles == on {
        return;
    }
    s.show_profiles = on;
    let fx = if on {
        // The ones fetched without asking; strangers' come when their chat is opened.
        let keys = s.auto_profile_keys();
        s.ask_profiles(&keys)
    } else {
        Effects::default()
    };
    st.slot_profile.show_profiles = Some(on);
    if let Err(e) = st.store.save_profile(st.slot, &st.slot_profile) {
        toast(ui, &format!("Not saved: {e}"));
    }
    apply(ui, st, fx);
    save_now(st);
    refresh_all(ui, st);
}

// ---- chats ---------------------------------------------------------------------------------

fn open_chat(ui: &AppWindow, st: &mut State, peer: PublicKey) {
    if st.open_peer != Some(peer) {
        st.reply_to = None;
    }
    // The composer is shared by every chat: park the text typed for the previous chat and
    // bring back this one's (and drop an error that was about the other chat).
    let hex = peer.to_hex();
    if st.draft_peer.as_deref() != Some(hex.as_str()) {
        let app = ui.global::<App>();
        if let Some(old) = st.draft_peer.take() {
            let text = app.get_compose_text().to_string();
            if text.trim().is_empty() {
                st.drafts.remove(&old);
            } else {
                st.drafts.insert(old, text);
            }
        }
        app.set_compose_text(st.drafts.remove(&hex).unwrap_or_default().into());
        app.set_compose_error(SharedString::new());
        st.draft_peer = Some(hex);
    }
    st.open_peer = Some(peer);
    st.selected = None;
    // A "new message" toast is about a chat the user was not in; opening a chat answers it.
    ui.global::<App>().set_toast(SharedString::new());
    if let Some(s) = st.session.as_mut() {
        if s.inbox.mark_read(&peer.to_hex()) {
            schedule_save();
        }
    }
    preload_kept(st, &peer);
    refresh_thread(ui, st);
    refresh_chats(ui, st);
    ui.global::<App>().set_page(PAGE_THREAD);
    // Strangers' profiles are not fetched on arrival (the phone would learn who wrote to
    // you); opening their chat asks for it.
    want_profile(ui, st, &peer);
}

/// Ask the phone for someone's profile (once per link; nothing if profiles are off).
fn want_profile(ui: &AppWindow, st: &mut State, pk: &PublicKey) {
    let Some(s) = st.session.as_mut() else { return };
    let fx = s.ask_profiles(&[pk.to_hex()]);
    apply(ui, st, fx);
}

fn open_contact(ui: &AppWindow, st: &mut State, peer: PublicKey) {
    let Some(s) = st.session.as_ref() else { return };
    let app = ui.global::<App>();
    st.contact_peer = Some(peer);
    // Unsaved: offer the name from their profile (saving it is still the user's call).
    let name = match s.contact(&peer) {
        Some(c) => c.name.clone(),
        None => s.profile_label(&peer).map(|n| n.chars().take(nip_ping_core::session::MAX_NAME_CHARS).collect::<String>().trim().to_string()).unwrap_or_default(),
    };
    app.set_contact_name(name.into());
    app.set_contact_error(SharedString::new());
    // The contact page's back button returns to this chat.
    st.open_peer = Some(peer);
    st.selected = None;
    refresh_contact(ui, st);
    refresh_thread(ui, st);
    app.set_page(PAGE_CONTACT);
    want_profile(ui, st, &peer);
}

fn start_new_chat(ui: &AppWindow, st: &mut State, npub: &str) {
    let app = ui.global::<App>();
    st.new_chat = NewChat::default();
    app.set_new_name(SharedString::new());
    app.set_new_npub_error(SharedString::new());
    app.set_new_error(SharedString::new());
    app.set_new_npub(npub.into());
    new_input(ui, st, npub);
    app.set_page(PAGE_NEW_CHAT);
}

/// The new-chat field changed: what is in it?
fn new_input(ui: &AppWindow, st: &mut State, text: &str) {
    let app = ui.global::<App>();
    let t = text.trim();
    app.set_new_npub_error(SharedString::new());
    let before = st.new_chat.pk;
    let kind = if t.is_empty() {
        st.new_chat.pk = None;
        st.new_chat.address = None;
        0
    } else if let Ok(pk) = parse_pubkey(t) {
        let fresh = st.new_chat.pk != Some(pk) || st.new_chat.address.is_some();
        st.new_chat.pk = Some(pk);
        st.new_chat.address = None;
        if fresh {
            want_profile(ui, st, &pk);
        }
        1
    } else if let Some(q) = profile::parse_nip05(t) {
        // Keep a finished lookup while the field still says the same address.
        if st.new_chat.address.as_deref() != Some(q.as_str()) {
            st.new_chat.pk = None;
            st.new_chat.address = None;
            st.new_chat.looking = false;
        }
        2
    } else {
        st.new_chat.pk = None;
        st.new_chat.address = None;
        3
    };
    // Someone else now: a name filled in from the previous person's profile goes.
    if st.new_chat.pk != before && !st.new_chat.prefill.is_empty() && app.get_new_name() == st.new_chat.prefill.as_str() {
        app.set_new_name(SharedString::new());
        st.new_chat.prefill.clear();
    }
    app.set_new_kind(kind);
    refresh_new_chat(ui, st);
}

fn lookup(ui: &AppWindow, st: &mut State) {
    let app = ui.global::<App>();
    let text = app.get_new_npub().to_string();
    let Some(s) = st.session.as_mut() else { return };
    match s.lookup_nip05(&text) {
        Ok(fx) => {
            st.new_chat.address = profile::parse_nip05(&text);
            st.new_chat.pk = None;
            st.new_chat.looking = true;
            app.set_new_npub_error(SharedString::new());
            apply(ui, st, fx);
            refresh_new_chat(ui, st);
        }
        Err(e) => app.set_new_npub_error(sentence(e).into()),
    }
}

fn nip05_answer(ui: &AppWindow, st: &mut State, q: &str, r: Result<PublicKey, String>) {
    if st.new_chat.address.as_deref() != Some(q) {
        return;
    }
    st.new_chat.looking = false;
    let app = ui.global::<App>();
    match r {
        Ok(pk) => {
            st.new_chat.pk = Some(pk);
            app.set_new_npub_error(SharedString::new());
        }
        Err(e) => {
            st.new_chat.address = None;
            app.set_new_npub_error(format!("No Nostr key found for {}: {}", profile::nip05_display(q), sentence(e)).into());
        }
    }
    refresh_new_chat(ui, st);
}

fn add_chat(ui: &AppWindow, st: &mut State, input: &str, name: &str) {
    let app = ui.global::<App>();
    app.set_new_npub_error(SharedString::new());
    app.set_new_error(SharedString::new());
    let input = input.trim();
    let pk = match parse_pubkey(input) {
        Ok(pk) => pk,
        Err(_) => match (profile::parse_nip05(input), st.new_chat.pk) {
            // An address that was looked up: its key.
            (Some(q), Some(pk)) if st.new_chat.address.as_deref() == Some(q.as_str()) => pk,
            (Some(_), _) => {
                lookup(ui, st);
                return;
            }
            (None, _) => {
                app.set_new_npub_error("That is not an npub, nprofile, hex key or name@domain address.".into());
                return;
            }
        },
    };
    let Some(s) = st.session.as_mut() else { return };
    let name = name.trim();
    if !name.is_empty() {
        if let Err(e) = s.save_contact(pk, name) {
            app.set_new_error(sentence(e).into());
            return;
        }
        schedule_save();
    }
    open_chat(ui, st, pk);
    want_profile(ui, st, &pk);
}

fn import_nsec(ui: &AppWindow, st: &mut State, text: &str) {
    let app = ui.global::<App>();
    let sk = match parse_secret(text) {
        Ok(sk) => sk,
        Err(_) => {
            app.set_import_error("That is not a Nostr secret key (nsec1… or 64 hex characters).".into());
            return;
        }
    };
    let pk = sk.public_key().to_hex();
    if let Some(slot) = st.accounts.list.iter().find(|a| a.pk == pk).map(|a| a.slot) {
        app.set_import_text(SharedString::new());
        app.set_import_error(SharedString::new());
        switch_account(ui, st, slot);
        toast(ui, "That account is already on this Passport. Switched to it.");
        return;
    }
    match add_account(ui, st, Some(encode_nsec(&sk))) {
        Ok(()) => {
            app.set_import_text(SharedString::new());
            app.set_import_error(SharedString::new());
            toast(ui, "Your Nostr account is on this Passport now. Chats from other clients arrive as the phone syncs.");
        }
        Err(e) => app.set_import_error(format!("Could not add it: {e}").into()),
    }
}

/// Text from a QR, whichever path it came by (Scan button or Launcher hand-off).
fn handle_scanned(ui: &AppWindow, st: &mut State, mode: i32, text: &str) {
    match mode {
        // A scanned secret never goes into the text field, so it is not left on screen.
        SCAN_NSEC => import_nsec(ui, st, text),
        _ => match parse_pubkey(text) {
            Ok(pk) => {
                if let Some(s) = st.session.as_ref() {
                    if s.contact(&pk).is_some() {
                        open_chat(ui, st, pk);
                        return;
                    }
                }
                start_new_chat(ui, st, &encode_npub(&pk));
            }
            Err(_) => show_error(ui, "Not an npub", "Scan the QR of someone's Nostr public key (npub). Their Nostr app shows it on their profile.", true),
        },
    }
}

/// Open a Launcher hand-off that waited for an account.
fn take_pending_scan(ui: &AppWindow, st: &mut State) {
    if st.session.is_none() {
        return;
    }
    if let Some(text) = PENDING_SCAN.with(|p| p.borrow_mut().take()) {
        // A confirmation from the page being left must not stay armed over the new one.
        ui.global::<Confirm>().set_action(SharedString::new());
        handle_scanned(ui, st, SCAN_NPUB, &text);
    }
}

fn scan(ui: &AppWindow, mode: i32) {
    let options = ScanQrOptions {
        header_title: if mode == SCAN_NSEC { "Scan nsec".into() } else { "Scan npub".into() },
        message: if mode == SCAN_NSEC { "Point at the QR of your nsec".into() } else { "Point at someone's npub QR".into() },
        ..ScanQrOptions::default()
    };
    match open_qr_scanner::<gui_permissions::GuiPermissions>(options) {
        Ok(Some(ScanQrResult::Qr { data, .. })) => match String::from_utf8(data) {
            Ok(text) => with_app(|ui, st| handle_scanned(ui, st, mode, text.trim())),
            Err(_) => show_error(ui, "Unreadable QR", "That QR does not contain text.", true),
        },
        Ok(Some(ScanQrResult::Ur2 { .. })) => show_error(ui, "Unexpected QR", "That is a UR code, not a Nostr key.", true),
        Ok(_) => {} // cancelled
        Err(e) => {
            log::error!("scanner: {e:?}");
            show_error(ui, "Scanner unavailable", "The camera could not be opened.", false);
        }
    }
}

// ---- messages: reactions, replies, pictures ------------------------------------------------

fn select(ui: &AppWindow, st: &mut State, id: &str) {
    st.selected = if st.selected.as_deref() == Some(id) || id.is_empty() { None } else { Some(id.to_string()) };
    refresh_selection(ui, st);
}

fn react(ui: &AppWindow, st: &mut State, emoji: &str) {
    let (Some(id), Some(peer)) = (st.selected.take(), st.open_peer) else { return };
    let Some(s) = st.session.as_mut() else { return };
    let me = s.me.pk.to_hex();
    let same = s.inbox.find_message(&id).is_some_and(|m| m.reactions.iter().any(|r| r.by == me && bare_emoji(&r.emoji) == bare_emoji(emoji)));
    if same {
        refresh_selection(ui, st);
        return;
    }
    match s.react(&peer, &id, emoji) {
        Ok(fx) => apply(ui, st, fx),
        Err(e) => toast(ui, &format!("Reaction not sent: {}", sentence(e))),
    }
    refresh_thread(ui, st);
}

fn start_reply(ui: &AppWindow, st: &mut State) {
    let Some(id) = st.selected.take() else { return };
    st.reply_to = Some(id);
    ui.global::<App>().set_compose_error(SharedString::new());
    refresh_thread(ui, st);
    focus_composer(ui);
}

fn send(ui: &AppWindow, st: &mut State, text: &str) {
    let app = ui.global::<App>();
    let (Some(s), Some(peer)) = (st.session.as_mut(), st.open_peer) else { return };
    let r = match st.reply_to.as_deref() {
        Some(parent) => s.send_reply(&peer, text, parent),
        None => s.send(&peer, text),
    };
    match r {
        Ok(fx) => {
            // The field empties and keeps the cursor, ready for the next message.
            st.reply_to = None;
            app.set_compose_text(SharedString::new());
            app.set_compose_error(SharedString::new());
            apply(ui, st, fx);
            refresh_thread(ui, st);
        }
        Err(e) => app.set_compose_error(sentence(e).into()),
    }
}

/// Remember a loaded picture (in memory only, unless kept).
#[allow(clippy::too_many_arguments)]
fn remember_picture(st: &mut State, msg: String, w: u32, h: u32, rgba: &[u8], kept: bool, frames: Vec<nip_ping_core::media::Frame>, file: Vec<u8>, ext: &'static str) -> bool {
    let Some(buf) = picture_buffer(w, h, rgba) else { return false };
    let img = Image::from_rgba8(buf.clone());
    let frames: Vec<(Image, u32)> = frames.iter().filter_map(|f| picture_buffer(w, h, &f.rgba).map(|b| (Image::from_rgba8(b), f.ms))).collect();
    let anim = !frames.is_empty();
    st.pics.insert(msg.clone(), Pic::Ready { w, h, buf, img, kept, frames, file, ext });
    st.pic_order.retain(|m| *m != msg);
    st.pic_order.push_back(msg.clone());
    // Bounded memory: older animations keep their first frame only (the one on the viewer stays).
    if anim {
        let older: Vec<String> = st
            .pic_order
            .iter()
            .rev()
            .filter(|m| matches!(st.pics.get(*m), Some(Pic::Ready { frames, .. }) if !frames.is_empty()))
            .skip(MAX_ANIMS_IN_MEMORY)
            .filter(|m| st.viewer.as_deref() != Some(m.as_str()))
            .cloned()
            .collect();
        for m in older {
            if let Some(Pic::Ready { frames, file, .. }) = st.pics.get_mut(&m) {
                frames.clear();
                file.clear();
            }
        }
    }
    // Bounded memory: drop the oldest (it comes back from storage if kept, or on a tap).
    while st.pic_order.len() > MAX_PICS_IN_MEMORY {
        let Some(old) = st.pic_order.pop_front() else { break };
        if st.viewer.as_deref() == Some(old.as_str()) {
            st.pic_order.push_back(old);
            continue;
        }
        st.pics.remove(&old);
    }
    true
}

/// Pictures of messages that are gone (deleted by their author, expired: NIP-40 disappearing
/// messages) leave memory and the viewer at once, instead of staying on screen with a Keep
/// button.
fn prune_pictures(ui: &AppWindow, st: &mut State) {
    let Some(s) = st.session.as_ref() else { return };
    let gone: Vec<String> = st.pics.keys().filter(|id| s.inbox.find_message(id).is_none()).cloned().collect();
    if gone.is_empty() {
        return;
    }
    for id in &gone {
        st.pics.remove(id);
        st.pic_order.retain(|m| m != id);
    }
    if st.viewer.as_ref().is_some_and(|v| gone.contains(v)) {
        st.viewer = None;
        let app = ui.global::<App>();
        app.set_viewer_picture(Image::default());
        if app.get_page() == PAGE_PICTURE {
            app.set_page(PAGE_THREAD);
        }
    }
}

fn picture_arrived(st: &mut State, p: Picture) {
    if let Some(e) = p.error {
        st.pics.insert(p.msg, Pic::Failed(e));
        return;
    }
    if !remember_picture(st, p.msg.clone(), p.w, p.h, &p.rgba, false, p.frames, p.file, p.ext) {
        st.pics.insert(p.msg, Pic::Failed("not a picture".into()));
    }
}

/// Kept pictures of this chat, newest first, read from storage so they show without the phone.
fn preload_kept(st: &mut State, peer: &PublicKey) {
    let Some(s) = st.session.as_ref() else { return };
    let Some(chat) = s.inbox.chat(&peer.to_hex()) else { return };
    let ids: Vec<String> = chat.messages.iter().rev().filter(|m| st.kept.iter().any(|k| k.msg == m.id) && !st.pics.contains_key(&m.id)).take(KEPT_PRELOAD).map(|m| m.id.clone()).collect();
    for id in ids {
        load_kept(st, &id);
    }
}

fn load_kept(st: &mut State, msg: &str) -> bool {
    let Some(s) = st.session.as_ref() else { return false };
    let id = data_id(&st.seed, &s.me.pk);
    match st.store.load_picture(&id, msg) {
        Ok(Some((w, h, rgba))) => remember_picture(st, msg.to_string(), w, h, &rgba, true, Vec::new(), Vec::new(), ""),
        Ok(None) => false,
        Err(e) => {
            log::warn!("storage: {e}");
            false
        }
    }
}

/// Is message `msg` a video (a kind 15 video, or a video link)?
fn is_video(s: &Session, msg: &str) -> bool {
    s.inbox.find_message(msg).is_some_and(|m| match &m.file {
        Some(f) => f.label() == "Video",
        None => display::picture_url(&m.text).is_none() && display::video_url(&m.text).is_some(),
    })
}

fn load_media(ui: &AppWindow, st: &mut State, msg: &str) {
    if matches!(st.pics.get(msg), Some(Pic::Loading | Pic::Ready { .. })) {
        return;
    }
    if st.kept.iter().any(|k| k.msg == msg) && load_kept(st, msg) {
        refresh_thread(ui, st);
        return;
    }
    let Some(s) = st.session.as_mut() else { return };
    let video = is_video(s, msg);
    let Some((file, url)) = s.inbox.find_message(msg).map(|m| (m.file.is_some(), display::picture_url(&m.text).or_else(|| display::video_url(&m.text)))) else { return };
    let r = match (file, url) {
        (true, _) => s.load_file(msg),
        (false, Some(url)) if video => s.load_video(msg, &url),
        (false, Some(url)) => s.load_picture(msg, &url),
        (false, None) => return,
    };
    match r {
        Ok(fx) => {
            st.pics.insert(msg.to_string(), Pic::Loading);
            apply(ui, st, fx);
        }
        Err(e) => toast(ui, &sentence(e)),
    }
    refresh_thread(ui, st);
}

/// The viewer's next animation frame, then the one after (stops when the viewer closes).
fn next_frame() {
    with_app(|ui, st| {
        let on_viewer = ui.global::<App>().get_page() == PAGE_PICTURE;
        let Some(Pic::Ready { frames, .. }) = st.viewer.as_ref().and_then(|v| st.pics.get(v)) else { return };
        if !on_viewer || frames.is_empty() {
            return;
        }
        st.viewer_frame = (st.viewer_frame + 1) % frames.len();
        let (img, ms) = frames[st.viewer_frame].clone();
        ui.global::<App>().set_viewer_picture(img);
        ANIM_TIMER.with(|t| t.start(TimerMode::SingleShot, Duration::from_millis(u64::from(ms)), next_frame));
    })
}

fn open_picture(ui: &AppWindow, st: &mut State, msg: &str) {
    if !matches!(st.pics.get(msg), Some(Pic::Ready { .. })) {
        return;
    }
    st.selected = None;
    st.viewer_frame = 0;
    ANIM_TIMER.with(|t| t.stop());
    st.viewer = Some(msg.to_string());
    refresh_viewer(ui, st);
    refresh_selection(ui, st);
    ui.global::<App>().set_page(PAGE_PICTURE);
}

fn keep_picture(ui: &AppWindow, st: &mut State, msg: &str) {
    let Some(s) = st.session.as_ref() else { return };
    let id = data_id(&st.seed, &s.me.pk);
    let now = s.now();
    let Some(Pic::Ready { w, h, buf, kept, .. }) = st.pics.get(msg) else { return };
    if *kept || st.kept_unreadable {
        return;
    }
    if let Err(e) = st.store.save_picture(&id, msg, *w, *h, buf.as_bytes()) {
        toast(ui, &format!("Not kept: {e}"));
        return;
    }
    st.kept.push(store::Kept { msg: msg.to_string(), at: now });
    // At most MAX_KEPT: the oldest goes (and the toast says so).
    let mut dropped = 0;
    while st.kept.len() > store::MAX_KEPT {
        dropped += 1;
        let old = st.kept.remove(0);
        if let Err(e) = st.store.remove_picture(&id, &old.msg) {
            log::warn!("storage: {e}");
        }
        if let Some(Pic::Ready { kept, .. }) = st.pics.get_mut(&old.msg) {
            *kept = false;
        }
    }
    if let Err(e) = st.store.save_kept(&id, &st.kept) {
        toast(ui, &format!("Not kept: {e}"));
        return;
    }
    if let Some(Pic::Ready { kept, .. }) = st.pics.get_mut(msg) {
        *kept = true;
    }
    st.selected = None;
    refresh_thread(ui, st);
    refresh_viewer(ui, st);
    if dropped > 0 {
        toast(ui, &format!("Kept, sealed. {} pictures at most: the oldest kept one is forgotten on this Passport.", store::MAX_KEPT));
    } else {
        // The picture page says it under Save; elsewhere (the message actions) a toast does.
        if ui.global::<App>().get_page() != PAGE_PICTURE {
            toast(ui, "Kept in Sanctm, sealed. It shows without the phone.");
        }
    }
}

/// `2026-10-30` and `231405` (UTC) for file names.
fn utc_stamp(t: u64) -> (String, String) {
    let (days, secs) = ((t / 86_400) as i64, t % 86_400);
    // Days since 1970-01-01 -> civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (format!("{y:04}-{m:02}-{d:02}"), format!("{:02}{:02}{:02}", secs / 3600, secs / 60 % 60, secs % 60))
}

/// Where a folder picked in Files lives, for the file system.
/// The places a picture copy can go, as the file browser, the file system and the UI name them.
const PLACES: [(&str, PickLocation, Location, &str); 3] = [
    ("user", PickLocation::Internal, Location::User, "Passport storage"),
    ("airlock", PickLocation::Airlock, Location::Airlock, "the Airlock"),
    ("usb", PickLocation::External, Location::Usb, "the USB drive"),
];

fn place_of(l: PickLocation) -> usize {
    match l {
        PickLocation::Internal => 0,
        PickLocation::Airlock => 1,
        PickLocation::External => 2,
    }
}

/// "Passport storage / Pictures" for the Save sheet's one-tap row.
fn save_label(p: &store::SavePlace) -> String {
    let Some((_, _, _, name)) = PLACES.iter().find(|(key, ..)| *key == p.place) else { return String::new() };
    let mut name = name.strip_prefix("the ").unwrap_or(name).to_string();
    if let Some(first) = name.get(..1) {
        name = first.to_uppercase() + &name[1..];
    }
    let dir = p.dir.trim_matches('/');
    if dir.is_empty() { name } else { format!("{name} / {dir}") }
}

/// "Save a copy": the picture is written as a PNG to a folder the user picks in KeyOS's own
/// file browser, opened on one place only (Passport storage, the Airlock or a USB drive), or
/// straight to the folder used last time (`place` 3).
fn save_picture_copy(ui: &AppWindow, msg: &str, place: i32) {
    // Make the file first, so no state is borrowed while the file browser is up: a GIF or a
    // video as it was sent (so it still moves), a still picture as a PNG.
    let mut job = None;
    with_app(|ui, st| {
        let Some(Pic::Ready { w, h, buf, file, ext, .. }) = st.pics.get(msg) else { return };
        let now = st.session.as_ref().map_or(0, |s| s.now());
        let (day, time) = utc_stamp(now);
        let short: String = msg.chars().take(8).collect();
        if !file.is_empty() {
            job = Some((file.clone(), format!("sanctm-{day}-{time}-{short}.{ext}")));
            return;
        }
        match nip_ping_core::media::encode_png(*w, *h, buf.as_bytes()) {
            Ok(png) => job = Some((png, format!("sanctm-{day}-{time}-{short}.png"))),
            Err(e) => toast(ui, &format!("Not saved: {}", sentence(&e.to_string()))),
        }
    });
    let Some((png, name)) = job else { return };
    let picked = if place == 3 {
        let mut last = None;
        with_app(|_, st| {
            last = st.slot_profile.last_save.as_ref().and_then(|p| PLACES.iter().position(|(key, ..)| *key == p.place).map(|i| (p.dir.clone(), i)));
        });
        last
    } else {
        let Some(&(_, pick, ..)) = PLACES.get(place.clamp(0, 2) as usize) else { return };
        let options = SelectFileOptions::default()
            .with_dir_selection_mode(true)
            .with_start_location(pick)
            .with_allowed_locations(AllowedLocations::specific([pick]));
        match select_file::<gui_permissions::GuiPermissions>(options) {
            Ok(Some(result)) => result.files().first().map(|(dir, l)| (dir.clone(), place_of(*l))),
            Ok(None) => None, // cancelled
            Err(e) => {
                log::error!("file browser: {e:?}");
                toast(ui, "Files could not be opened, so nothing was saved.");
                return;
            }
        }
    };
    let Some((dir, i)) = picked else { return };
    let (key, _, location, place_name) = PLACES[i];
    let dir = dir.trim_end_matches('/');
    let path = if dir.is_empty() { name.clone() } else { format!("{dir}/{name}") };
    with_app(|ui, st| {
        // The file browser can stay up for minutes: a disappearing message (NIP-40) may have
        // expired, or its sender deleted it (kind 5), meanwhile. Then no unsealed copy is made.
        let still_here = st.session.as_ref().is_some_and(|s| {
            let now = s.now();
            s.inbox.find_message(msg).is_some_and(|m| m.expires.map_or(true, |e| e > now))
        }) && matches!(st.pics.get(msg), Some(Pic::Ready { .. }));
        if !still_here {
            toast(ui, "Not saved: the message is gone (deleted or disappeared).");
            return;
        }
        match st.store.export(&path, location, &png) {
            Ok(()) => {
                toast(ui, &format!("Saved to {place_name}{}{}: {name}. It is not sealed there.", if dir.is_empty() { "" } else { " / " }, dir.trim_start_matches('/')));
                st.slot_profile.last_save = Some(store::SavePlace { place: key.to_string(), dir: dir.to_string() });
                if let Err(e) = st.store.save_profile(st.slot, &st.slot_profile) {
                    log::warn!("storage: {e}");
                }
                refresh_viewer(ui, st);
            }
            // While a computer is plugged in it holds the Airlock, and the Passport cannot write.
            Err(e) if location == Location::Airlock => toast(ui, &format!("Not saved: {e}. If the Passport is plugged into a computer, unplug it first.")),
            Err(e) => toast(ui, &format!("Not saved: {e}.")),
        }
    });
}

fn forget_picture(ui: &AppWindow, st: &mut State, msg: &str) {
    let Some(s) = st.session.as_ref() else { return };
    let id = data_id(&st.seed, &s.me.pk);
    if !st.kept.iter().any(|k| k.msg == msg) {
        return;
    }
    if let Err(e) = st.store.remove_picture(&id, msg) {
        log::warn!("storage: {e}");
    }
    st.kept.retain(|k| k.msg != msg);
    if let Err(e) = st.store.save_kept(&id, &st.kept) {
        toast(ui, &format!("Not saved: {e}"));
    }
    if let Some(Pic::Ready { kept, .. }) = st.pics.get_mut(msg) {
        *kept = false;
    }
    refresh_thread(ui, st);
    refresh_viewer(ui, st);
}

// ---- startup -------------------------------------------------------------------------------

fn boot(ui: &AppWindow, fs: std::sync::Arc<FileSystem>) {
    let seed = match device::app_seed() {
        Ok(seed) => seed,
        Err(e) => {
            show_error(ui, "Sanctm cannot start", &e, false);
            return;
        }
    };
    if seed.is_dev() {
        ui.global::<App>().set_seed_note("Simulator dev seed: the app seed was refused, so keys use a fixed test seed.".into());
    }
    let seed = seed.bytes();

    let link = transport::Link::start(
        |frame| {
            let _ = slint_keyos_platform::slint::invoke_from_event_loop(move || {
                with_app(|ui, st| {
                    match nip_ping_core::link::parse(&frame) {
                        Ok(nip_ping_core::link::Inbound::Hello { .. }) => st.companion_hello = Some(frame.clone()),
                        Ok(nip_ping_core::link::Inbound::Relays { .. }) => st.companion_relays = Some(frame.clone()),
                        _ => {}
                    }
                    let Some(s) = st.session.as_mut() else { return };
                    let fx = s.handle_frame(&frame);
                    apply(ui, st, fx);
                });
            });
        },
        |status| {
            let _ = slint_keyos_platform::slint::invoke_from_event_loop(move || {
                with_app(|ui, st| set_link(ui, st, status));
            });
        },
    );

    let state = Rc::new(RefCell::new(State {
        store: store::Store::new(fs, seed),
        seed,
        accounts: store::Accounts::default(),
        accounts_unreadable: false,
        slot: 0,
        slot_profile: store::Profile::default(),
        session: None,
        link,
        link_up: false,
        link_conn: 0,
        link_text: String::new(),
        open_peer: None,
        contact_peer: None,
        storage_note: String::new(),
        contacts_unreadable: false,
        inbox_unreadable: false,
        people_unreadable: false,
        kept_unreadable: false,
        companion_hello: None,
        companion_relays: None,
        people_saved: 0,
        avatars: RefCell::new(HashMap::new()),
        pics: HashMap::new(),
        pic_order: VecDeque::new(),
        kept: Vec::new(),
        selected: None,
        reply_to: None,
        viewer: None,
        viewer_frame: 0,
        new_chat: NewChat::default(),
        drafts: HashMap::new(),
        draft_peer: None,
    }));
    APP.with(|cell| *cell.borrow_mut() = Some((state, ui.as_weak())));
    with_app(start);

    // Keep "5 min ago" honest, and let disappearing messages (NIP-40) go.
    CLOCK_TIMER.with(|t| {
        t.start(TimerMode::Repeated, Duration::from_secs(30), || {
            with_app(|ui, st| {
                if st.session.as_mut().is_some_and(|s| s.tick()) {
                    if st.selected.as_ref().is_some_and(|id| st.session.as_ref().is_some_and(|s| s.inbox.find_message(id).is_none())) {
                        st.selected = None;
                    }
                    prune_pictures(ui, st);
                    schedule_save();
                }
                refresh_chats(ui, st);
                if on_thread(ui) {
                    refresh_thread(ui, st);
                }
            })
        })
    });
}

fn app_main(cx: AppContext, ui: AppWindow) {
    log_server::init_wait(env!("CARGO_CRATE_NAME")).unwrap();
    log::set_max_level(log::LevelFilter::Info);
    log::info!("Sanctm starting");

    theme::init(&ui);
    // http://localhost picture links: only the simulator's local test servers use them.
    nip_ping_core::profile::allow_local_http(SIM);
    ui.global::<Utils>().on_qrcode(|data, dark, light| {
        if data.is_empty() {
            slint_keyos_platform::slint::Image::default()
        } else {
            slint_keyos_platform::qrcode::render(data.as_str(), dark, light)
        }
    });
    let app = ui.global::<App>();
    app.set_sim(SIM);
    app.set_page(PAGE_LOADING);

    let actions = ui.global::<Actions>();
    actions.on_byte_len(|s| s.len() as i32);
    actions.on_is_blank(|s| s.trim().is_empty());

    actions.on_create_identity(|| {
        with_app(|ui, st| {
            if let Err(e) = add_account(ui, st, None) {
                show_error(ui, "Could not save", &e, false);
            }
        })
    });

    actions.on_add_account(|| {
        with_app(|ui, st| match add_account(ui, st, None) {
            Ok(()) => {
                let label = st.accounts.get(st.slot).map(account_label).unwrap_or_default();
                toast(ui, &format!("{label} created on this Passport. Share its npub from Me."));
            }
            Err(e) => toast(ui, &format!("Not added: {e}")),
        })
    });

    actions.on_import_nsec(|text| with_app(|ui, st| import_nsec(ui, st, &text)));

    actions.on_start_import(|| {
        with_app(|ui, _| {
            let app = ui.global::<App>();
            app.set_import_text(SharedString::new());
            app.set_import_error(SharedString::new());
            app.set_page(PAGE_IMPORT);
        })
    });

    let weak = ui.as_weak();
    let weak_save = weak.clone();
    actions.on_save_picture_copy(move |id, place| {
        // Files is a modal app: open it outside this callback, with no borrow held.
        let weak = weak_save.clone();
        Timer::single_shot(Duration::ZERO, move || {
            if let Some(ui) = weak.upgrade() {
                save_picture_copy(&ui, &id, place);
            }
        });
    });
    actions.on_scan(move |mode| {
        // Run the modal scanner outside this callback so no borrow is held while it is up.
        let weak = weak.clone();
        Timer::single_shot(Duration::ZERO, move || {
            if let Some(ui) = weak.upgrade() {
                scan(&ui, mode);
            }
        });
    });

    actions.on_open_chat(|hex| {
        with_app(|ui, st| {
            if let Ok(pk) = PublicKey::from_hex(&hex) {
                open_chat(ui, st, pk);
            }
        })
    });

    actions.on_chat_with(|hex| {
        with_app(|ui, st| {
            if let Ok(pk) = PublicKey::from_hex(&hex) {
                open_chat(ui, st, pk);
            }
        })
    });

    actions.on_back_to_chats(|| {
        with_app(|ui, st| {
            st.open_peer = None;
            st.selected = None;
            st.reply_to = None;
            refresh_chats(ui, st);
            ui.global::<App>().set_page(PAGE_CHATS);
        })
    });

    actions.on_start_compose(|| {
        with_app(|ui, st| {
            st.selected = None;
            refresh_thread(ui, st);
            let app = ui.global::<App>();
            app.set_compose_error(SharedString::new());
            app.set_page(PAGE_THREAD);
            focus_composer(ui);
        })
    });

    actions.on_send(|text| with_app(|ui, st| send(ui, st, &text)));

    actions.on_new_chat(|| {
        with_app(|ui, st| {
            refresh_chats(ui, st);
            start_new_chat(ui, st, "");
        })
    });

    actions.on_new_input(|text| with_app(|ui, st| new_input(ui, st, &text)));
    actions.on_lookup(|| with_app(lookup));
    actions.on_add_chat(|npub, name| with_app(|ui, st| add_chat(ui, st, &npub, &name)));

    actions.on_open_contact(|hex| {
        with_app(|ui, st| {
            if let Ok(pk) = PublicKey::from_hex(&hex) {
                open_contact(ui, st, pk);
            }
        })
    });

    actions.on_save_contact(|| {
        with_app(|ui, st| {
            let app = ui.global::<App>();
            let (Some(s), Some(pk)) = (st.session.as_mut(), st.contact_peer) else { return };
            let name = app.get_contact_name();
            if let Err(e) = s.save_contact(pk, &name) {
                app.set_contact_error(sentence(e).into());
                return;
            }
            schedule_save();
            open_chat(ui, st, pk);
            want_profile(ui, st, &pk);
        })
    });

    actions.on_remove_contact(|| {
        with_app(|ui, st| {
            let (Some(s), Some(pk)) = (st.session.as_mut(), st.contact_peer) else { return };
            s.remove_contact(&pk);
            schedule_save();
            refresh_chats(ui, st);
            open_contact(ui, st, pk);
        })
    });

    actions.on_delete_chat(|| {
        with_app(|ui, st| {
            let (Some(s), Some(pk)) = (st.session.as_mut(), st.contact_peer) else { return };
            // Remembered as deletes (like a kind 5), so a phone that replays the wraps once
            // the seen-wrap list has rolled over (3000 junk wraps) cannot bring the chat back.
            let (peer, me) = (pk.to_hex(), s.me.pk.to_hex());
            let (outs, ins): (Vec<(bool, String)>, Vec<(bool, String)>) =
                s.inbox.chat(&peer).map(|c| c.messages.iter().map(|m| (m.out, m.id.clone())).collect::<Vec<_>>()).unwrap_or_default().into_iter().partition(|(out, _)| *out);
            s.inbox.remove_chat(&peer);
            s.inbox.delete_by(&ins.into_iter().map(|(_, id)| id).collect::<Vec<_>>(), &peer, false);
            s.inbox.delete_by(&outs.into_iter().map(|(_, id)| id).collect::<Vec<_>>(), &me, true);
            st.open_peer = None;
            schedule_save();
            refresh_chats(ui, st);
            ui.global::<App>().set_page(PAGE_CHATS);
        })
    });

    actions.on_show_me(|| {
        with_app(|ui, st| {
            refresh_status(ui, st);
            ui.global::<App>().set_page(PAGE_ME);
        })
    });

    actions.on_show_accounts(|| {
        with_app(|ui, st| {
            // The key shown on the remove page goes from the screen with it.
            let app = ui.global::<App>();
            app.set_remove_nsec(SharedString::new());
            app.set_remove_nsec_chunked(SharedString::new());
            KEY_TIMER.with(|t| t.stop());
            refresh_accounts(ui, st);
            ui.global::<App>().set_page(PAGE_ACCOUNTS);
        })
    });

    actions.on_switch_account(|slot| with_app(|ui, st| switch_account(ui, st, slot.max(0) as u32)));
    actions.on_start_remove(|slot| with_app(|ui, st| start_remove(ui, st, slot.max(0) as u32)));
    actions.on_show_remove_key(|| with_app(show_remove_key));
    actions.on_remove_account(|slot| with_app(|ui, st| remove_account(ui, st, slot.max(0) as u32)));
    actions.on_fetch_history(|| {
        with_app(|ui, st| {
            let Some(s) = st.session.as_mut() else { return };
            let fx = s.fetch_all_history();
            apply(ui, st, fx);
            toast(ui, "Asking the relays for every message. This can take a while.");
        })
    });

    actions.on_set_show_profiles(|on| with_app(|ui, st| set_show_profiles(ui, st, on)));

    actions.on_retry_load(|| with_app(start));

    actions.on_go_home(|| {
        with_app(|ui, st| {
            st.open_peer = None;
            st.contact_peer = None;
            st.selected = None;
            refresh_chats(ui, st);
            let page = if st.session.is_some() {
                PAGE_CHATS
            } else if st.accounts.list.is_empty() || st.accounts_unreadable {
                PAGE_ONBOARDING
            } else {
                // An account that did not load: pick or add another one.
                refresh_accounts(ui, st);
                PAGE_ACCOUNTS
            };
            ui.global::<App>().set_page(page);
        })
    });

    actions.on_select(|id| with_app(|ui, st| select(ui, st, &id)));
    actions.on_react(|emoji| with_app(|ui, st| react(ui, st, &emoji)));
    actions.on_start_reply(|| with_app(start_reply));
    actions.on_cancel_reply(|| {
        with_app(|ui, st| {
            st.reply_to = None;
            refresh_thread(ui, st);
        })
    });
    actions.on_retry_send(|| {
        with_app(|ui, st| {
            let Some(id) = st.selected.take() else { return };
            let Some(s) = st.session.as_mut() else { return };
            let fx = s.retry(&id);
            apply(ui, st, fx);
            refresh_thread(ui, st);
        })
    });
    actions.on_load_media(|id| with_app(|ui, st| load_media(ui, st, &id)));
    actions.on_open_picture(|id| with_app(|ui, st| open_picture(ui, st, &id)));
    actions.on_close_picture(|| {
        with_app(|ui, st| {
            st.viewer = None;
            ANIM_TIMER.with(|t| t.stop());
            refresh_thread(ui, st);
            ui.global::<App>().set_page(PAGE_THREAD);
        })
    });
    actions.on_keep_picture(|id| with_app(|ui, st| keep_picture(ui, st, &id)));
    actions.on_forget_picture(|id| with_app(|ui, st| forget_picture(ui, st, &id)));

    // Launcher QR hand-off: an npub scanned from the Launcher opens a new chat with it. One
    // that arrives while Sanctm is still starting (or before it has an account) waits.
    let gui = cx.gui.clone();
    cx.set_input_handler(move |input| {
        if input.msg != InputMessage::NavigationFocused {
            return;
        }
        let Ok(Some(bytes)) = gui.navigate_pending() else { return };
        let Some(MatchedQrResult { scan_result, .. }) = MatchedQrResult::from_slice(&bytes) else { return };
        let ScanQrResult::Qr { data, .. } = scan_result else { return };
        if let Ok(text) = String::from_utf8(data) {
            PENDING_SCAN.with(|p| *p.borrow_mut() = Some(text.trim().to_string()));
            with_app(take_pending_scan);
        }
    });

    // The app seed prompt needs a running event loop (SDK gotcha #24): boot from a timer.
    let weak = ui.as_weak();
    let fs = cx.fs.clone();
    Timer::single_shot(Duration::from_millis(50), move || {
        if let Some(ui) = weak.upgrade() {
            boot(&ui, fs);
        }
    });

    ui.run().expect("UI running");
}
