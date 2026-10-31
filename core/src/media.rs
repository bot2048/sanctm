// SPDX-License-Identifier: GPL-3.0-or-later

//! Opening a NIP-17 kind 15 file on the Passport: check the downloaded ciphertext against `x`,
//! decrypt it (AES-256-GCM, tag appended), and decode it as a picture shrunk to fit a chat
//! bubble. The phone only ever handles the encrypted bytes. Animated GIF and WebP pictures
//! keep their frames (shrunk the same way), so the Passport can play them.
//!
//! Interop, as found in other clients: keys and nonces are hex (sometimes base64); nonces are
//! 12 bytes (Coracle, Nostur) or 16 (Amethyst, 0xchat); older 0xchat used the nonce's hex text
//! itself as the nonce bytes; `file-type` may be wrong, so the format is taken from the bytes.

use aes_gcm::aead::consts::{U12, U16, U24, U32};
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::aes::Aes256;
use aes_gcm::{AesGcm, Nonce};
use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use sha2::{Digest, Sha256};

use crate::nip17::FileMeta;
use crate::session::{PICTURE_H, PICTURE_W};
use crate::{Error, Result};

/// Pixels a picture may have before it is shrunk, and the memory its decoder may take: a
/// 12 MP photo decodes in about 36–48 MB on the Passport. Bigger ones: open them on a phone.
const MAX_PIXELS: u64 = 12_000_000;
const MAX_SIDE: u32 = 8192;
const MAX_DECODE_ALLOC: u64 = 64 * 1024 * 1024;
/// An animation keeps at most this many frames, and at most this many bytes of them (RGBA):
/// a longer one plays its start, then loops.
pub const MAX_FRAMES: usize = 60;
const MAX_ANIM_BYTES: usize = 20 * 1024 * 1024;

pub struct Rgba {
    pub w: u32,
    pub h: u32,
    /// The first frame (the whole picture, for a still one).
    pub rgba: Vec<u8>,
    /// Every frame of an animated picture, with how long it shows (ms); empty for a still one.
    pub frames: Vec<Frame>,
}

pub struct Frame {
    pub rgba: Vec<u8>,
    pub ms: u32,
}

fn bytes_of(s: &str) -> Vec<Vec<u8>> {
    let s = s.trim();
    let mut v = Vec::new();
    if let Ok(b) = hex::decode(s) {
        v.push(b);
    }
    if let Ok(b) = B64.decode(s) {
        v.push(b);
    }
    v
}

fn decrypt_with(key: &[u8], nonce: &[u8], ct: &[u8]) -> Option<Vec<u8>> {
    macro_rules! try_size {
        ($n:ty) => {{
            let c = AesGcm::<Aes256, $n>::new_from_slice(key).ok()?;
            c.decrypt(Nonce::<$n>::from_slice(nonce), ct).ok()
        }};
    }
    match nonce.len() {
        12 => try_size!(U12),
        16 => try_size!(U16),
        24 => try_size!(U24),
        32 => try_size!(U32),
        _ => None,
    }
}

/// Ciphertext (as served) -> plaintext file.
pub fn decrypt_file(meta: &FileMeta, ct: &[u8]) -> Result<Vec<u8>> {
    if !meta.algo.is_empty() && !meta.algo.eq_ignore_ascii_case("aes-gcm") {
        return Err(Error::Frame("unknown file encryption"));
    }
    if let Some(x) = &meta.x {
        if !hex::encode(Sha256::digest(ct)).eq_ignore_ascii_case(x) {
            return Err(Error::Frame("the file on the server is not the one that was sent"));
        }
    }
    let keys: Vec<Vec<u8>> = bytes_of(&meta.key).into_iter().filter(|k| k.len() == 32).collect();
    let mut nonces = bytes_of(&meta.nonce);
    nonces.push(meta.nonce.trim().as_bytes().to_vec()); // old 0xchat: the hex text itself
    for k in &keys {
        for n in &nonces {
            if let Some(p) = decrypt_with(k, n, ct) {
                return Ok(p);
            }
        }
    }
    Err(Error::Frame("the file does not open with its key"))
}

fn limits() -> image::Limits {
    let mut l = image::Limits::default();
    l.max_alloc = Some(MAX_DECODE_ALLOC);
    l.max_image_width = Some(MAX_SIDE);
    l.max_image_height = Some(MAX_SIDE);
    l
}

/// Size that fits `PICTURE_W` x `PICTURE_H`, aspect kept, never enlarged.
fn fit(w: u32, h: u32) -> (u32, u32) {
    let scale = f64::min(1.0, f64::min(f64::from(PICTURE_W) / f64::from(w), f64::from(PICTURE_H) / f64::from(h)));
    (((f64::from(w) * scale).round() as u32).max(1), ((f64::from(h) * scale).round() as u32).max(1))
}

/// Frames of an animation, shrunk to fit, until `MAX_FRAMES` or `MAX_ANIM_BYTES`.
fn collect_frames<'a>(frames: image::Frames<'a>) -> Result<Rgba> {
    let mut out: Vec<Frame> = Vec::new();
    let (mut tw, mut th, mut bytes) = (0, 0, 0usize);
    for f in frames {
        let f = f.map_err(|_| Error::Frame("damaged picture"))?;
        let ms = {
            let (n, d) = f.delay().numer_denom_ms();
            let ms = n.checked_div(d).unwrap_or(0);
            // Like browsers: a delay under 20 ms means "as fast as the file was made for", 100 ms.
            if ms < 20 { 100 } else { ms.min(10_000) }
        };
        let buf = f.into_buffer();
        let (w, h) = buf.dimensions();
        if out.is_empty() {
            if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
                return Err(Error::Frame("picture too large to open on Passport"));
            }
            (tw, th) = fit(w, h);
        }
        let small = if (w, h) == (tw, th) { buf } else { image::imageops::thumbnail(&buf, tw, th) };
        bytes += small.len();
        if bytes > MAX_ANIM_BYTES {
            break;
        }
        out.push(Frame { rgba: small.into_raw(), ms });
        if out.len() >= MAX_FRAMES {
            break;
        }
    }
    let Some(first) = out.first() else { return Err(Error::Frame("damaged picture")) };
    let rgba = first.rgba.clone();
    if out.len() == 1 {
        out.clear();
    }
    Ok(Rgba { w: tw, h: th, rgba, frames: out })
}

/// An animated GIF or WebP -> its frames, if it is one.
fn decode_animation(file: &[u8], format: image::ImageFormat) -> Option<Result<Rgba>> {
    use image::AnimationDecoder;
    let cur = std::io::Cursor::new(file);
    match format {
        image::ImageFormat::Gif => {
            let mut d = image::codecs::gif::GifDecoder::new(cur).ok()?;
            image::ImageDecoder::set_limits(&mut d, limits()).ok()?;
            Some(collect_frames(d.into_frames()))
        }
        image::ImageFormat::WebP => {
            let d = image::codecs::webp::WebPDecoder::new(cur).ok()?;
            if !d.has_animation() {
                return None;
            }
            Some(collect_frames(d.into_frames()))
        }
        _ => None,
    }
}

/// A picture file -> RGBA that fits `PICTURE_W` x `PICTURE_H` (every frame of an animation).
pub fn decode_picture(file: &[u8]) -> Result<Rgba> {
    let format = image::guess_format(file).map_err(|_| Error::Frame("not a picture this Passport can show"))?;
    if !matches!(format, image::ImageFormat::Jpeg | image::ImageFormat::Png | image::ImageFormat::WebP | image::ImageFormat::Gif) {
        return Err(Error::Frame("open this picture on a phone app"));
    }
    if let Some(anim) = decode_animation(file, format) {
        return anim;
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(file), format);
    reader.limits(limits());
    let (w, h) = reader.into_dimensions().map_err(|_| Error::Frame("damaged picture"))?;
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(Error::Frame("picture too large to open on Passport"));
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(file), format);
    reader.limits(limits());
    let img = reader.decode().map_err(|_| Error::Frame("picture too large to open on Passport, or damaged"))?;
    let (w, h) = (img.width(), img.height());
    let (tw, th) = fit(w, h);
    // Shrink in the decoded format first: no full-size RGBA copy.
    let small = if (tw, th) == (w, h) { img } else { img.thumbnail_exact(tw, th) };
    Ok(Rgba { w: tw, h: th, rgba: small.into_rgba8().into_raw(), frames: Vec::new() })
}

/// Downloaded ciphertext of a kind 15 message -> picture to show.
pub fn open_file(meta: &FileMeta, ct: &[u8]) -> Result<Rgba> {
    if !meta.is_image() && !meta.mime.is_empty() && !meta.mime.starts_with("application/octet") {
        return Err(Error::Frame("open this file on a phone app"));
    }
    decode_picture(&decrypt_file(meta, ct)?)
}

/// A shown picture as a PNG file, for saving a copy outside the app.
pub fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) * 4 != rgba.len() as u64 {
        return Err(Error::Frame("damaged picture"));
    }
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec()).ok_or(Error::Frame("damaged picture"))?;
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).map_err(|_| Error::Frame("could not make a PNG"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shown_picture_saves_as_a_png_that_opens_again() {
        let rgba: Vec<u8> = (0..6 * 4 * 4).map(|i| (i * 7 % 256) as u8).collect();
        let file = encode_png(6, 4, &rgba).unwrap();
        assert_eq!(&file[..8], b"\x89PNG\r\n\x1a\n");
        let back = decode_picture(&file).unwrap();
        assert_eq!((back.w, back.h), (6, 4));
        assert_eq!(back.rgba, rgba);
        assert!(encode_png(6, 4, &rgba[1..]).is_err());
        assert!(encode_png(0, 4, &[]).is_err());
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn encrypt(key: &[u8; 32], nonce: &[u8], plain: &[u8]) -> Vec<u8> {
        match nonce.len() {
            12 => AesGcm::<Aes256, U12>::new_from_slice(key).unwrap().encrypt(Nonce::<U12>::from_slice(nonce), plain).unwrap(),
            16 => AesGcm::<Aes256, U16>::new_from_slice(key).unwrap().encrypt(Nonce::<U16>::from_slice(nonce), plain).unwrap(),
            24 => AesGcm::<Aes256, U24>::new_from_slice(key).unwrap().encrypt(Nonce::<U24>::from_slice(nonce), plain).unwrap(),
            _ => unreachable!(),
        }
    }

    fn meta(key: &str, nonce: &str, x: Option<String>) -> FileMeta {
        FileMeta { url: "https://b/f".into(), mime: "image/png".into(), algo: "aes-gcm".into(), key: key.into(), nonce: nonce.into(), x, size: None, dim: None }
    }

    #[test]
    fn opens_files_from_each_client_style() {
        let key = [7u8; 32];
        let file = png(800, 600);
        // Coracle / Nostur: 12-byte hex nonce; Amethyst / 0xchat: 16-byte hex nonce; base64.
        for nonce in [vec![1u8; 12], vec![2u8; 16]] {
            let ct = encrypt(&key, &nonce, &file);
            let x = hex::encode(Sha256::digest(&ct));
            let pic = open_file(&meta(&hex::encode(key), &hex::encode(&nonce), Some(x)), &ct).unwrap();
            assert_eq!((pic.w, pic.h), (360, 270), "shrunk to fit the bubble, aspect kept");
            assert_eq!(pic.rgba.len(), 360 * 270 * 4);
            let pic = open_file(&meta(&B64.encode(key), &B64.encode(&nonce), None), &ct).unwrap();
            assert_eq!(pic.w, 360);
        }
        // Old 0xchat: the nonce's hex text used as the nonce bytes.
        let nonce_hex = hex::encode([3u8; 12]);
        let ct = encrypt(&key, nonce_hex.as_bytes(), &file);
        assert!(open_file(&meta(&hex::encode(key), &nonce_hex, None), &ct).is_ok());
        // Small pictures are not enlarged.
        let small = png(40, 30);
        let ct = encrypt(&key, &[1u8; 12], &small);
        let pic = open_file(&meta(&hex::encode(key), &hex::encode([1u8; 12]), None), &ct).unwrap();
        assert_eq!((pic.w, pic.h), (40, 30));
    }

    #[test]
    fn refuses_tampered_or_wrong_files() {
        let key = [7u8; 32];
        let ct = encrypt(&key, &[1u8; 12], &png(10, 10));
        let good = meta(&hex::encode(key), &hex::encode([1u8; 12]), None);
        let mut bad = ct.clone();
        bad[5] ^= 1;
        assert!(open_file(&good, &bad).is_err(), "GCM tag");
        assert!(open_file(&meta(&hex::encode([8u8; 32]), &hex::encode([1u8; 12]), None), &ct).is_err(), "wrong key");
        assert!(open_file(&meta(&hex::encode(key), &hex::encode([1u8; 12]), Some(hex::encode([0u8; 32]))), &ct).is_err(), "x mismatch");
        let mut video = good.clone();
        video.mime = "video/mp4".into();
        assert!(open_file(&video, &ct).is_err(), "not shown on Passport");
        let not_pic = encrypt(&key, &[1u8; 12], b"just some text, not an image");
        assert!(open_file(&good, &not_pic).is_err());
        // A tiny file that claims huge dimensions is refused before decoding.
        let mut huge = png(10, 10);
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(decode_picture(&huge).is_err());
    }

    fn gif(frames: u32, w: u16, h: u16) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut out);
            enc.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for i in 0..frames {
                let img = image::RgbaImage::from_fn(u32::from(w), u32::from(h), |x, _| image::Rgba([(x * 40 + i * 30) as u8, 0, 0, 255]));
                let f = image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(80, 1));
                enc.encode_frame(f).unwrap();
            }
        }
        out
    }

    #[test]
    fn animated_gifs_keep_their_frames() {
        let pic = decode_picture(&gif(5, 720, 400)).unwrap();
        assert_eq!((pic.w, pic.h), (360, 200), "shrunk like a still picture");
        assert_eq!(pic.frames.len(), 5);
        assert!(pic.frames.iter().all(|f| f.rgba.len() == 360 * 200 * 4 && f.ms == 80));
        assert_eq!(pic.rgba, pic.frames[0].rgba);
        // Long ones stop at MAX_FRAMES; a one-frame GIF is a still picture.
        assert_eq!(decode_picture(&gif(70, 16, 16)).unwrap().frames.len(), MAX_FRAMES);
        let still = decode_picture(&gif(1, 16, 16)).unwrap();
        assert!(still.frames.is_empty() && still.rgba.len() == 16 * 16 * 4);
        // An encrypted GIF (kind 15) opens the same way.
        let key = [9u8; 32];
        let ct = encrypt(&key, &[4u8; 12], &gif(3, 32, 32));
        let mut m = meta(&hex::encode(key), &hex::encode([4u8; 12]), None);
        m.mime = "image/gif".into();
        assert_eq!(open_file(&m, &ct).unwrap().frames.len(), 3);
    }
}
