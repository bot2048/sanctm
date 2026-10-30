// SPDX-FileCopyrightText: 2026 Foundation Devices, Inc. <hello@foundation.xyz>
// SPDX-License-Identifier: GPL-3.0-or-later
//
// NIP-19 bech32 entities. npub/nsec from the Passport Prime Nostr Signer
// (logic/nostr-core/src/bech32.rs); nprofile decoding and a tolerant pubkey parser added.

use bech32::primitives::decode::CheckedHrpstring;
use bech32::{Bech32, Checksum, Hrp};

/// Plain bech32 (BIP-173 checksum, never bech32m) with NIP-19's 5000-character limit, so long
/// nprofiles decode as they do in nostr-tools.
enum Nip19Bech32 {}
impl Checksum for Nip19Bech32 {
    type MidstateRepr = u32;
    const CODE_LENGTH: usize = 5000;
    const CHECKSUM_LENGTH: usize = 6;
    const GENERATOR_SH: [u32; 5] = [0x3b6a_57b2, 0x2650_8e6d, 0x1ea1_19fa, 0x3d42_33dd, 0x2a14_62b3];
    const TARGET_RESIDUE: u32 = 1;
}

use crate::{Error, PublicKey, Result, SecretKey};

fn encode(hrp: &'static str, data: &[u8]) -> Result<String> {
    let hrp = Hrp::parse(hrp).map_err(|e| Error::Bech32(e.to_string()))?;
    bech32::encode::<Bech32>(hrp, data).map_err(|e| Error::Bech32(e.to_string()))
}

fn decode(expected: &'static str, s: &str) -> Result<Vec<u8>> {
    let checked = CheckedHrpstring::new::<Nip19Bech32>(s).map_err(|e| Error::Bech32(e.to_string()))?;
    if checked.hrp().as_str() != expected {
        return Err(Error::Bech32Hrp { expected, got: checked.hrp().as_str().to_string() });
    }
    Ok(checked.byte_iter().collect())
}

pub fn encode_npub(pk: &PublicKey) -> String { encode("npub", pk.as_bytes()).expect("32 bytes always encode") }

pub fn decode_npub(s: &str) -> Result<PublicKey> { PublicKey::from_slice(&decode("npub", s)?) }

pub fn encode_nsec(sk: &SecretKey) -> String { encode("nsec", sk.as_bytes()).expect("32 bytes always encode") }

pub fn decode_nsec(s: &str) -> Result<SecretKey> {
    let mut bytes = decode("nsec", s)?;
    let out = SecretKey::from_slice(&bytes);
    zeroize::Zeroize::zeroize(&mut bytes);
    out
}

/// An existing Nostr account's secret key, as people have it: `nsec1…`, `nostr:nsec1…`
/// (any case, e.g. from an uppercase QR) or 64 hex characters. The returned error never
/// echoes the input.
pub fn parse_secret(input: &str) -> Result<SecretKey> {
    let s = input.trim();
    let s = s.strip_prefix("nostr:").or_else(|| s.strip_prefix("NOSTR:")).unwrap_or(s);
    let mut lower = s.to_ascii_lowercase();
    let out = if lower.starts_with("nsec1") {
        decode_nsec(&lower).map_err(|_| Error::InvalidKey)
    } else if lower.len() == 64 && lower.bytes().all(|b| b.is_ascii_hexdigit()) {
        SecretKey::from_hex(&lower).map_err(|_| Error::InvalidKey)
    } else {
        Err(Error::InvalidKey)
    };
    zeroize::Zeroize::zeroize(&mut lower);
    out
}

/// nprofile: TLV list, type 0 = 32-byte pubkey, type 1 = relay (ignored here).
pub fn decode_nprofile(s: &str) -> Result<PublicKey> {
    let data = decode("nprofile", s)?;
    let mut i = 0;
    while i + 2 <= data.len() {
        let (t, l) = (data[i], data[i + 1] as usize);
        let v = data.get(i + 2..i + 2 + l).ok_or(Error::Bech32("truncated TLV".into()))?;
        if t == 0 {
            return PublicKey::from_slice(v);
        }
        i += 2 + l;
    }
    Err(Error::NotAPubkey)
}

/// Accepts what people actually paste or scan: `npub1…`, `nostr:npub1…`, `nprofile1…`,
/// or 64 hex characters. Surrounding whitespace and an uppercase QR (`NPUB1…`) are fine.
pub fn parse_pubkey(input: &str) -> Result<PublicKey> {
    let s = input.trim();
    let s = s.strip_prefix("nostr:").or_else(|| s.strip_prefix("NOSTR:")).unwrap_or(s);
    let lower = s.to_ascii_lowercase();
    if lower.starts_with("npub1") {
        return decode_npub(&lower).map_err(|_| Error::NotAPubkey);
    }
    if lower.starts_with("nprofile1") {
        return decode_nprofile(&lower).map_err(|_| Error::NotAPubkey);
    }
    if lower.len() == 64 && lower.bytes().all(|b| b.is_ascii_hexdigit()) {
        return PublicKey::from_hex(&lower).map_err(|_| Error::NotAPubkey);
    }
    Err(Error::NotAPubkey)
}

/// `npub1abcd…wxyz` for lists.
pub fn short_npub(pk: &PublicKey) -> String {
    let n = encode_npub(pk);
    format!("{}…{}", &n[..10], &n[n.len() - 6..])
}

/// The npub in groups of five for reading aloud / comparing: `npub1 80cvv 07tjd …`.
pub fn chunked_npub(pk: &PublicKey) -> String {
    let n = encode_npub(pk);
    let (head, rest) = n.split_at(5);
    let mut out = String::from(head);
    for c in rest.as_bytes().chunks(5) {
        out.push(' ');
        out.push_str(core::str::from_utf8(c).unwrap_or_default());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // NIP-19 published vectors.
    const NPUB_HEX: &str = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
    const NPUB_STR: &str = "npub180cvv07tjdrrgpa0j7j7tmnyl2yr6yr7l8j4s3evf6u64th6gkwsyjh6w6";
    const NSEC_HEX: &str = "67dea2ed018072d675f5415ecfaed7d2597555e202d85b3d65ea4e58d2d92ffa";
    const NSEC_STR: &str = "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5";
    const NPROFILE: &str = "nprofile1qqsrhuxx8l9ex335q7he0f09aej04zpazpl0ne2cgukyawd24mayt8gpp4mhxue69uhhytnc9e3k7mgpz4mhxue69uhkg6nzv9ejuumpv34kytnrdaksjlyr9p";

    #[test]
    fn npub_and_nsec_vectors() {
        let pk = PublicKey::from_hex(NPUB_HEX).unwrap();
        assert_eq!(encode_npub(&pk), NPUB_STR);
        assert_eq!(decode_npub(NPUB_STR).unwrap(), pk);
        let sk = SecretKey::from_hex(NSEC_HEX).unwrap();
        assert_eq!(encode_nsec(&sk), NSEC_STR);
        assert_eq!(decode_nsec(NSEC_STR).unwrap().as_bytes(), sk.as_bytes());
    }

    #[test]
    fn nprofile_vector() {
        assert_eq!(decode_nprofile(NPROFILE).unwrap().to_hex(), NPUB_HEX);
    }

    #[test]
    fn tolerant_parser() {
        let want = PublicKey::from_hex(NPUB_HEX).unwrap();
        for s in [
            NPUB_STR.to_string(),
            format!("  nostr:{NPUB_STR}\n"),
            NPUB_STR.to_ascii_uppercase(),
            NPROFILE.to_string(),
            NPUB_HEX.to_string(),
            NPUB_HEX.to_ascii_uppercase(),
        ] {
            assert_eq!(parse_pubkey(&s).unwrap(), want, "{s}");
        }
        for bad in ["", "npub1xyz", NSEC_STR, &NPUB_HEX[..63], "hello"] {
            assert!(parse_pubkey(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn secret_parser() {
        let want = SecretKey::from_hex(NSEC_HEX).unwrap();
        for s in [
            NSEC_STR.to_string(),
            format!(" nostr:{NSEC_STR}\n"),
            NSEC_STR.to_ascii_uppercase(),
            NSEC_HEX.to_string(),
            NSEC_HEX.to_ascii_uppercase(),
        ] {
            assert_eq!(parse_secret(&s).unwrap().as_bytes(), want.as_bytes());
        }
        for bad in ["", NPUB_STR, "nsec1qqqq", &NSEC_HEX[..62], &"0".repeat(64), "correct horse battery staple"] {
            assert!(parse_secret(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn display_helpers() {
        let pk = PublicKey::from_hex(NPUB_HEX).unwrap();
        assert_eq!(short_npub(&pk), "npub180cvv…yjh6w6");
        assert!(chunked_npub(&pk).starts_with("npub1 80cvv 07tjd"));
        assert_eq!(chunked_npub(&pk).replace(' ', ""), NPUB_STR);
    }
}
