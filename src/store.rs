// SPDX-License-Identifier: GPL-3.0-or-later

//! Sealed persistence in this app's KeyOS AppData directory.
//!
//! Every file is JSON (or, for kept pictures, raw pixels) sealed with AES-256-GCM under a key
//! derived from the app seed (`nip_ping_core::vault`), then written with `durable_file_write`,
//! so a crash leaves the old or the new file, and a copied disk image shows neither messages
//! nor contacts. Derived keys are never stored; an imported nsec is, sealed like everything
//! else.
//!
//! Files:
//! * `accounts.json`: the accounts on this Passport (slots, which one is in use, a cached copy
//!   of each account's own Nostr profile for the account list).
//! * `profile-<slot>.json`: which identity a slot uses (an imported nsec, or a NIP-06 index)
//!   and its settings.
//! * `contacts-<tag>.json`, `inbox-<tag>.json`, `people-<tag>.json`, `kept-<tag>.json`: one
//!   identity's contacts, chats, Nostr profiles and kept-picture list (tag: a keyed hash of
//!   the public key, see `vault::file_tag`).
//! * `pic-<tag>.bin`: one kept picture.

use std::sync::Arc;

use nip_ping_core::profile::{Profile as NostrProfile, Profiles};
use nip_ping_core::{open_file, seal_file, Contact, Inbox};
use serde::{Deserialize, Serialize};
use slint_keyos_platform::fs::{Error as FsError, Location};

use crate::FileSystem;

/// Most accounts on one Passport.
pub const MAX_ACCOUNTS: usize = 10;
/// Slot numbers looked at when the account list is rebuilt, and handed out: more than
/// MAX_ACCOUNTS, because a slot whose file cannot be read is never reused.
pub const SLOTS: u32 = 2 * MAX_ACCOUNTS as u32;
/// Most pictures kept per account; the oldest goes first.
pub const MAX_KEPT: usize = 30;

#[derive(Serialize, Deserialize, Default, Clone)]
/// Which identity a slot uses, and its settings.
pub struct Profile {
    /// Imported key (bech32 nsec). None: the key is derived from the app seed.
    #[serde(default)]
    pub nsec: Option<String>,
    /// NIP-06 account index of a derived key. None: the slot number (as before accounts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// Fetch and show Nostr profiles (names, photos). None: on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_profiles: Option<bool>,
    /// Relays the user published as DM relays (kind 10050): the only ones AUTH is signed for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dm_relays: Vec<String>,
    /// The slot was removed (its key forgotten). Never loaded again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
    /// Where this account last saved a picture copy, offered again as one tap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_save: Option<SavePlace>,
}

/// A folder a picture copy was saved to: the place ("user", "airlock", "usb") and the folder
/// in it, as the file browser returned it.
#[derive(Serialize, Deserialize, Clone)]
pub struct SavePlace {
    pub place: String,
    pub dir: String,
}

/// One account in the registry.
#[derive(Serialize, Deserialize, Clone)]
pub struct Account {
    /// `profile-<slot>.json`.
    pub slot: u32,
    /// Hex public key, cached so the list needs no key derivation.
    pub pk: String,
    #[serde(default)]
    pub imported: bool,
    /// NIP-06 index of a derived key.
    #[serde(default)]
    pub index: u32,
    /// The account's own Nostr profile (name, photo), cached for the account list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub me: Option<NostrProfile>,
}

/// The accounts on this Passport.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Accounts {
    /// Slot in use.
    pub current: u32,
    pub list: Vec<Account>,
}

impl Accounts {
    pub fn get(&self, slot: u32) -> Option<&Account> { self.list.iter().find(|a| a.slot == slot) }

    pub fn get_mut(&mut self, slot: u32) -> Option<&mut Account> { self.list.iter_mut().find(|a| a.slot == slot) }

    /// Lowest NIP-06 index no derived account uses.
    pub fn free_index(&self) -> u32 { (0..).find(|i| !self.list.iter().any(|a| !a.imported && a.index == *i)).unwrap_or(0) }
}

/// A picture kept on this Passport, by message.
#[derive(Serialize, Deserialize, Clone)]
pub struct Kept {
    /// Rumor id of the message.
    pub msg: String,
    /// When it was kept (for dropping the oldest).
    pub at: u64,
}

pub struct Store {
    fs: Arc<FileSystem>,
    seed: [u8; 32],
}

const PIC_MAGIC: &[u8; 4] = b"NPIC";

impl Store {
    pub fn new(fs: Arc<FileSystem>, seed: [u8; 32]) -> Self { Self { fs, seed } }

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        match self.fs.durable_file_read(name, Location::AppData) {
            Ok(sealed) => open_file(&self.seed, name, &sealed).map(Some).map_err(|e| format!("{name}: {e}")),
            Err(FsError::FileNotFound) => Ok(None),
            Err(e) => Err(format!("{name}: read failed ({e:?})")),
        }
    }

    fn write(&self, name: &str, plain: &[u8]) -> Result<(), String> {
        let sealed = seal_file(&self.seed, name, plain).map_err(|e| format!("{name}: {e}"))?;
        self.fs.durable_file_write(name, Location::AppData, &sealed).map_err(|e| format!("{name}: write failed ({e:?})"))
    }

    fn load<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<Option<T>, String> {
        match self.read(name)? {
            Some(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("{name}: {e}")),
            None => Ok(None),
        }
    }

    fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let json = serde_json::to_vec(value).map_err(|e| format!("{name}: {e}"))?;
        self.write(name, &json)
    }

    /// A copy the user saves outside the app, in a folder they picked in Files: written as is,
    /// not sealed, so other apps (and a computer on the Airlock) can open it.
    pub fn export(&self, path: &str, location: Location, data: &[u8]) -> Result<(), String> {
        self.fs.durable_file_write(path, location, data).map_err(|e| match e {
            FsError::AccessDenied => "Sanctm may not write there".to_string(),
            FsError::FileNotFound => "that folder is gone".to_string(),
            _ => "could not write there".to_string(),
        })
    }

    /// Delete a file. One that is already gone is fine.
    fn remove(&self, name: &str) -> Result<(), String> {
        match self.fs.remove(name, Location::AppData) {
            Ok(()) | Err(FsError::FileNotFound) => Ok(()),
            Err(e) => Err(format!("{name}: delete failed ({e:?})")),
        }
    }

    // ---- accounts ----------------------------------------------------------------------

    pub fn load_accounts(&self) -> Result<Option<Accounts>, String> { self.load("accounts.json") }

    pub fn save_accounts(&self, a: &Accounts) -> Result<(), String> { self.save("accounts.json", a) }

    pub fn load_profile(&self, slot: u32) -> Result<Option<Profile>, String> { self.load(&format!("profile-{slot}.json")) }

    pub fn save_profile(&self, slot: u32, p: &Profile) -> Result<(), String> { self.save(&format!("profile-{slot}.json"), p) }

    /// Forget a slot: its key is overwritten first (so nothing is left even if the delete
    /// fails), then the file is deleted.
    pub fn forget_profile(&self, slot: u32) -> Result<(), String> {
        self.save_profile(slot, &Profile { removed: true, ..Profile::default() })?;
        self.remove(&format!("profile-{slot}.json"))
    }

    // Contacts and chats belong to an identity, not to a slot: switching accounts keeps each
    // identity's data apart, and an account added again finds its chats. `id` is
    // `vault::file_tag` of the public key.

    pub fn load_contacts(&self, id: &str) -> Result<Vec<Contact>, String> {
        Ok(self.load(&format!("contacts-{id}.json"))?.unwrap_or_default())
    }

    pub fn save_contacts(&self, id: &str, c: &[Contact]) -> Result<(), String> { self.save(&format!("contacts-{id}.json"), &c) }

    pub fn load_inbox(&self, id: &str) -> Result<Inbox, String> {
        Ok(self.load(&format!("inbox-{id}.json"))?.unwrap_or_default())
    }

    pub fn save_inbox(&self, id: &str, inbox: &Inbox) -> Result<(), String> { self.save(&format!("inbox-{id}.json"), inbox) }

    /// Nostr profiles (and their small avatars) of the people this identity chats with.
    pub fn load_people(&self, id: &str) -> Result<Profiles, String> {
        Ok(self.load(&format!("people-{id}.json"))?.unwrap_or_default())
    }

    pub fn save_people(&self, id: &str, p: &Profiles) -> Result<(), String> { self.save(&format!("people-{id}.json"), p) }

    // ---- kept pictures -----------------------------------------------------------------

    pub fn load_kept(&self, id: &str) -> Result<Vec<Kept>, String> {
        Ok(self.load(&format!("kept-{id}.json"))?.unwrap_or_default())
    }

    pub fn save_kept(&self, id: &str, k: &[Kept]) -> Result<(), String> { self.save(&format!("kept-{id}.json"), &k) }

    /// File of a kept picture: a keyed hash of identity and message, so the name shows
    /// neither.
    fn picture_file(&self, id: &str, msg: &str) -> String {
        format!("pic-{}.bin", nip_ping_core::vault::file_tag(&self.seed, &format!("{id}:{msg}")))
    }

    /// `NPIC`, width and height (u32 LE), then RGBA.
    pub fn save_picture(&self, id: &str, msg: &str, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
        let mut out = Vec::with_capacity(12 + rgba.len());
        out.extend_from_slice(PIC_MAGIC);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(rgba);
        self.write(&self.picture_file(id, msg), &out)
    }

    pub fn load_picture(&self, id: &str, msg: &str) -> Result<Option<(u32, u32, Vec<u8>)>, String> {
        let name = self.picture_file(id, msg);
        let Some(b) = self.read(&name)? else { return Ok(None) };
        if b.len() < 12 || &b[..4] != PIC_MAGIC {
            return Err(format!("{name}: not a kept picture"));
        }
        let w = u32::from_le_bytes(b[4..8].try_into().unwrap_or_default());
        let h = u32::from_le_bytes(b[8..12].try_into().unwrap_or_default());
        // Checked: usize is 32 bits on the device.
        let size = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)).and_then(|n| n.checked_add(12));
        if w == 0 || h == 0 || size != Some(b.len()) {
            return Err(format!("{name}: damaged picture"));
        }
        Ok(Some((w, h, b[12..].to_vec())))
    }

    pub fn remove_picture(&self, id: &str, msg: &str) -> Result<(), String> { self.remove(&self.picture_file(id, msg)) }
}
