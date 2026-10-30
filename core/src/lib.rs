// SPDX-License-Identifier: GPL-3.0-or-later

//! Sanctum for Passport Prime: everything that touches keys, messages or the link.
//!
//! * [`keys`], [`event`]  - secp256k1 keys, NIP-01 events, BIP-340 signatures
//! * [`nip19`]            - npub / nsec / nprofile
//! * [`nip44`]            - NIP-44 v2 (official vectors)
//! * [`nip59`]            - seal + gift wrap
//! * [`nip17`]            - private DMs, DM relay list (10050), relay AUTH (22242)
//! * [`identity`]         - KeyOS app seed -> NIP-06 key
//! * [`inbox`]            - chats as stored on the Passport
//! * [`link`]             - the Passport <-> companion frame protocol
//! * [`session`]          - the app's logic, UI-free
//! * [`vault`]            - AES-256-GCM sealed files keyed from the app seed
//! * [`display`]          - relative times and previews for the UI
//!
//! No KeyOS dependencies: `cargo test` runs anywhere.

pub mod display;
pub mod error;
pub mod event;
pub mod identity;
pub mod inbox;
pub mod keys;
pub mod link;
pub mod media;
pub mod nip17;
pub mod nip19;
pub mod nip44;
pub mod nip59;
pub mod profile;
pub mod session;
pub mod vault;

pub use error::Error;
pub use event::{Event, Rumor, UnsignedEvent};
pub use identity::{derive_from_app_seed, Identity};
pub use inbox::{Chat, Inbox, Message, Status};
pub use keys::{PublicKey, SecretKey};
pub use session::{Arrival, Contact, Effects, Picture, Session};
pub use vault::{open_file, seal_file};

pub type Result<T> = core::result::Result<T, Error>;
