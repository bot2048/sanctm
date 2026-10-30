// SPDX-License-Identifier: GPL-3.0-or-later

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid hex: {0}")]
    Hex(#[from] hex::FromHexError),

    #[error("invalid key length: expected {expected}, got {got}")]
    KeyLength { expected: usize, got: usize },

    #[error("invalid secp256k1 key")]
    InvalidKey,

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("nip-44: {0}")]
    Nip44(&'static str),

    #[error("base64: {0}")]
    Base64(#[from] base64::DecodeError),

    #[error("bech32: {0}")]
    Bech32(String),

    #[error("expected {expected}, got {got}")]
    Bech32Hrp { expected: &'static str, got: String },

    #[error("not a Nostr public key (npub, nprofile or 64 hex characters)")]
    NotAPubkey,

    #[error("event id does not match its contents")]
    EventIdMismatch,

    #[error("bad signature")]
    BadSignature,

    #[error("gift wrap: {0}")]
    Wrap(&'static str),

    #[error("seal author and message author differ (impersonation attempt)")]
    Impersonation,

    #[error("message is empty")]
    Empty,

    #[error("message too long: {got} bytes, max {max}")]
    TooLong { got: usize, max: usize },

    #[error("nip-06: {0}")]
    Nip06(&'static str),

    #[error("sealed storage: {0}")]
    Vault(&'static str),

    #[error("link frame: {0}")]
    Frame(&'static str),

    #[error("random number generator failed")]
    Rng,
}
