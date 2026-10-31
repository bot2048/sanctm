// SPDX-License-Identifier: GPL-3.0-or-later

//! Playing a video on the Passport, without sound: read the H.264 track of an MP4 file, decode
//! it (z264, pure Rust, no C), and keep about 12 frames a second, shrunk, until a memory budget
//! is used. The result plays like an animated picture. Everything here is untrusted input:
//! every offset and count is checked against the file before it is used.

use crate::media::{Frame, Rgba};
use crate::{Error, Result};

/// Frames shown per second (a 30 fps video keeps every third frame).
const TARGET_FPS: u32 = 12;
/// Size the frames are shrunk to fit.
const VIDEO_BOX: u32 = 320;
/// RGBA bytes of kept frames: about 9 s of a 16:9 video.
const MAX_VIDEO_RGBA: usize = 24 * 1024 * 1024;
/// Source frames decoded at most (decoding is the slow part on the device).
const MAX_DECODED: usize = 900;
const MAX_SAMPLES: usize = 200_000;
const MAX_PIXELS: u64 = 1920 * 1088;

const FAIL: Error = Error::Frame("this video does not play on Passport");

fn be32(b: &[u8], at: usize) -> Option<u32> { b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])) }

fn be64(b: &[u8], at: usize) -> Option<u64> { b.get(at..at + 8).map(|s| u64::from_be_bytes(s.try_into().unwrap_or_default())) }

/// The boxes in `data`: (type, payload).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let Some(size) = be32(data, at) else { break };
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap_or_default();
        let (head, len) = match size {
            0 => (8, data.len() - at),
            1 => match be64(data, at + 8).and_then(|n| usize::try_from(n).ok()) {
                Some(n) => (16, n),
                None => break,
            },
            n => (8, n as usize),
        };
        if len < head || at + len > data.len() {
            break;
        }
        out.push((kind, &data[at + head..at + len]));
        at += len;
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> { boxes(data).into_iter().find(|(k, _)| k == kind).map(|(_, p)| p) }

fn path<'a>(data: &'a [u8], kinds: &[&[u8; 4]]) -> Option<&'a [u8]> { kinds.iter().try_fold(data, |d, k| child(d, k)) }

/// The H.264 video track: its avcC, where each sample is, and the time per sample.
struct Track<'a> {
    avcc: &'a [u8],
    samples: Vec<(usize, usize)>,
    /// Frames per second, from the sample durations.
    fps: f64,
}

fn video_track(file: &[u8]) -> Result<Track<'_>> {
    let moov = child(file, b"moov").ok_or(FAIL)?;
    for (kind, trak) in boxes(moov) {
        if &kind != b"trak" {
            continue;
        }
        let Some(mdia) = child(trak, b"mdia") else { continue };
        if child(mdia, b"hdlr").and_then(|h| h.get(8..12)) != Some(b"vide") {
            continue;
        }
        let mdhd = child(mdia, b"mdhd").ok_or(FAIL)?;
        let timescale = if mdhd.first() == Some(&1) { be32(mdhd, 20) } else { be32(mdhd, 12) }.ok_or(FAIL)?;
        let stbl = path(mdia, &[b"minf", b"stbl"]).ok_or(FAIL)?;
        // stsd: one sample entry, avc1 / avc3, with its avcC after 78 bytes of fields.
        let stsd = child(stbl, b"stsd").ok_or(FAIL)?;
        let entry = boxes(stsd.get(8..).ok_or(FAIL)?).into_iter().next().ok_or(FAIL)?;
        if !matches!(&entry.0, b"avc1" | b"avc3") {
            return Err(Error::Frame("this video's format does not play on Passport"));
        }
        let avcc = child(entry.1.get(78..).ok_or(FAIL)?, b"avcC").ok_or(FAIL)?;

        let stsz = child(stbl, b"stsz").ok_or(FAIL)?;
        let (fixed, count) = (be32(stsz, 4).ok_or(FAIL)? as usize, be32(stsz, 8).ok_or(FAIL)? as usize);
        if count == 0 || count > MAX_SAMPLES {
            return Err(FAIL);
        }
        let sizes: Vec<usize> = if fixed != 0 { vec![fixed; count] } else { (0..count).map(|i| be32(stsz, 12 + 4 * i).map(|n| n as usize)).collect::<Option<_>>().ok_or(FAIL)? };
        let chunks: Vec<usize> = match (child(stbl, b"stco"), child(stbl, b"co64")) {
            (Some(b), _) => (0..be32(b, 4).ok_or(FAIL)? as usize).map(|i| be32(b, 8 + 4 * i).map(|n| n as usize)).collect::<Option<_>>().ok_or(FAIL)?,
            (None, Some(b)) => (0..be32(b, 4).ok_or(FAIL)? as usize).map(|i| be64(b, 8 + 8 * i).and_then(|n| usize::try_from(n).ok())).collect::<Option<_>>().ok_or(FAIL)?,
            _ => return Err(FAIL),
        };
        let stsc = child(stbl, b"stsc").ok_or(FAIL)?;
        let runs: Vec<(usize, usize)> = (0..be32(stsc, 4).ok_or(FAIL)? as usize)
            .map(|i| Some((be32(stsc, 8 + 12 * i)? as usize, be32(stsc, 12 + 12 * i)? as usize)))
            .collect::<Option<_>>()
            .ok_or(FAIL)?;
        // Samples, chunk by chunk.
        let mut samples = Vec::with_capacity(count);
        for (c, &start) in chunks.iter().enumerate() {
            let per = runs.iter().rev().find(|(first, _)| *first <= c + 1).map(|r| r.1).unwrap_or(0);
            let mut at = start;
            for _ in 0..per {
                let Some(&size) = sizes.get(samples.len()) else { break };
                if at.checked_add(size).is_none_or(|end| end > file.len()) {
                    return Err(FAIL);
                }
                samples.push((at, size));
                at += size;
            }
        }
        let stts = child(stbl, b"stts").ok_or(FAIL)?;
        let (mut n, mut ticks) = (0u64, 0u64);
        for i in 0..be32(stts, 4).ok_or(FAIL)? as usize {
            let (c, d) = (be32(stts, 8 + 8 * i).ok_or(FAIL)?, be32(stts, 12 + 8 * i).ok_or(FAIL)?);
            n += u64::from(c);
            ticks += u64::from(c) * u64::from(d);
        }
        let fps = if n > 0 && ticks > 0 && timescale > 0 { n as f64 * f64::from(timescale) / ticks as f64 } else { 30.0 };
        return Ok(Track { avcc, samples, fps: fps.clamp(1.0, 120.0) });
    }
    Err(Error::Frame("no video in this file"))
}

/// One decoded picture -> RGBA of `tw` x `th` (nearest sample, BT.601 limited range).
fn to_rgba(f: &z264::Frame, tw: u32, th: u32) -> Vec<u8> {
    let (w, h) = (f.width as usize, f.height as usize);
    let cw = w.div_ceil(2);
    let mut out = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th as usize {
        let y = ty * h / th as usize;
        for tx in 0..tw as usize {
            let x = tx * w / tw as usize;
            let c = i32::from(f.y.get(y * w + x).copied().unwrap_or(16)) - 16;
            let ci = (y / 2) * cw + x / 2;
            let d = i32::from(f.u.get(ci).copied().unwrap_or(128)) - 128;
            let e = i32::from(f.v.get(ci).copied().unwrap_or(128)) - 128;
            let px = |v: i32| ((v + 128) >> 8).clamp(0, 255) as u8;
            out.extend_from_slice(&[px(298 * c + 409 * e), px(298 * c - 100 * d - 208 * e), px(298 * c + 516 * d), 255]);
        }
    }
    out
}

/// Keeps every `every`-th decoded picture, shrunk, until the budget is used.
struct Keeper {
    every: usize,
    ms: u32,
    size: (u32, u32),
    decoded: usize,
    bytes: usize,
    full: bool,
    frames: Vec<Frame>,
}

impl Keeper {
    fn take(&mut self, pics: Vec<z264::Frame>) -> Result<()> {
        for p in pics {
            if self.full {
                return Ok(());
            }
            if self.decoded.is_multiple_of(self.every) {
                if self.frames.is_empty() {
                    if p.width == 0 || p.height == 0 || u64::from(p.width) * u64::from(p.height) > MAX_PIXELS {
                        return Err(Error::Frame("video too large to play on Passport"));
                    }
                    let k = f64::min(1.0, f64::from(VIDEO_BOX) / f64::from(p.width.max(p.height)));
                    self.size = (((f64::from(p.width) * k).round() as u32).max(1), ((f64::from(p.height) * k).round() as u32).max(1));
                }
                let rgba = to_rgba(&p, self.size.0, self.size.1);
                self.bytes += rgba.len();
                if self.bytes > MAX_VIDEO_RGBA {
                    self.full = true;
                    return Ok(());
                }
                self.frames.push(Frame { rgba, ms: self.ms });
            }
            self.decoded += 1;
            self.full |= self.decoded >= MAX_DECODED;
        }
        Ok(())
    }
}

/// An MP4 (H.264) file -> frames to play on the Passport. `Rgba::frames` has every kept frame.
pub fn decode_video(file: &[u8]) -> Result<Rgba> {
    let track = video_track(file)?;
    let cfg = z264::nal::parse_avcc_config(track.avcc).map_err(|_| FAIL)?;
    let mut dec = z264::OrderedDecoder::new();
    for nal in cfg.sps_nals.iter().chain(&cfg.pps_nals) {
        dec.decode_nal(nal).map_err(|_| FAIL)?;
    }
    let every = ((track.fps / f64::from(TARGET_FPS)).round() as usize).max(1);
    let ms = ((every as f64 * 1000.0 / track.fps).round() as u32).clamp(20, 1000);
    let mut k = Keeper { every, ms, size: (0, 0), decoded: 0, bytes: 0, full: false, frames: Vec::new() };
    for &(at, len) in &track.samples {
        for nal in z264::nal::parse_avcc(&file[at..at + len], cfg.length_size) {
            // A damaged picture is skipped; the next keyframe recovers.
            if let Ok(pics) = dec.decode_nal(&nal) {
                k.take(pics)?;
            }
        }
        if k.full {
            break;
        }
    }
    if !k.full {
        k.take(dec.flush())?;
    }
    let first = k.frames.first().ok_or(FAIL)?.rgba.clone();
    Ok(Rgba { w: k.size.0, h: k.size.1, rgba: first, frames: k.frames })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_what_is_not_an_mp4_video() {
        assert!(decode_video(b"").is_err());
        assert!(decode_video(b"\x00\x00\x00\x08moov").is_err());
        // A box that claims to be larger than the file.
        assert!(decode_video(b"\x00\x00\xff\xffmoov\x00\x00\x00\x00").is_err());
        let mut junk = vec![0u8; 4096];
        junk[3] = 16;
        junk[4..8].copy_from_slice(b"moov");
        assert!(decode_video(&junk).is_err());
    }

    #[test]
    fn plays_h264_videos_at_about_twelve_frames_a_second() {
        // 2 s at 30 fps, High profile with B-frames (display order differs from decode order).
        let v = decode_video(include_bytes!("../testdata/clip-high.mp4")).unwrap();
        assert_eq!((v.w, v.h), (96, 64), "small videos are not enlarged");
        assert_eq!(v.frames.len(), 20, "every third of 60 frames");
        assert!(v.frames.iter().all(|f| f.ms == 100 && f.rgba.len() == 96 * 64 * 4));
        // testsrc2 moves: frames differ, and are not blank.
        assert_ne!(v.frames[0].rgba, v.frames[10].rgba);
        assert!(v.rgba.chunks(4).any(|p| p[0] > 200) && v.rgba.chunks(4).any(|p| p[1] < 50));
        // Baseline, 24 fps: every other frame.
        let b = decode_video(include_bytes!("../testdata/clip-base.mp4")).unwrap();
        assert_eq!(b.frames.len(), 12);
        assert_eq!(b.frames[0].ms, 83);
        // A cut file stops cleanly: no panic, an error or fewer frames.
        let file = include_bytes!("../testdata/clip-high.mp4");
        let _ = decode_video(&file[..file.len() / 2]);
    }
}
