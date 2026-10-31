// SPDX-License-Identifier: GPL-3.0-or-later

//! Small text helpers for the UI, kept here so they are tested.

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian (H. Hinnant's algorithm).
pub fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// "now", "5 min", "3 h", "2 d", then a date. Relative, so no time zone is needed.
pub fn ago(now: u64, at: u64) -> String {
    let d = now.saturating_sub(at);
    match d {
        0..=59 => "now".into(),
        60..=3599 => format!("{} min", d / 60),
        3600..=86_399 => format!("{} h", d / 3600),
        86_400..=604_799 => format!("{} d", d / 86_400),
        _ => {
            let (y, m, day) = civil((at / 86_400) as i64);
            let (ny, _, _) = civil((now / 86_400) as i64);
            let month = MONTHS[(m - 1) as usize];
            if y == ny {
                format!("{month} {day}")
            } else {
                format!("{month} {day} {y}")
            }
        }
    }
}

/// Characters that can make text on screen read as something it is not: bidi overrides,
/// embeddings, isolates and marks, invisible separators and joiners, the BOM, line and
/// paragraph separators, interlinear annotation marks.
pub fn is_spoofing(c: char) -> bool {
    matches!(
        c,
        '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            // Blank-looking letters and marks: soft hyphen, combining grapheme joiner, Hangul
            // fillers, braille blank, tag characters.
            | '\u{00AD}'
            | '\u{034F}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{3164}'
            | '\u{FFA0}'
            | '\u{2800}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

fn is_line_break(c: char) -> bool { matches!(c, '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}') }

/// Text from someone else, made safe to show: line breaks of every kind become `\n`, tabs
/// become spaces, other control characters and the spoofing characters above are dropped.
/// A zero-width joiner or non-joiner between two non-ASCII letters or symbols is kept: it
/// builds emoji sequences (👩‍👩‍👧, 🏳️‍🌈) and is part of Persian and Indic spelling.
pub fn clean_text(s: &str) -> String { clean(s, true) }

fn clean(s: &str, joiners: bool) -> String {
    let joins = |c: Option<char>| c.is_some_and(|c| !c.is_ascii() && !c.is_whitespace() && !c.is_control() && !is_spoofing(c));
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' && chars.peek() == Some(&'\n') {
            continue; // CRLF -> LF
        }
        if is_line_break(c) {
            out.push('\n');
        } else if c == '\t' {
            out.push(' ');
        } else if matches!(c, '\u{200C}' | '\u{200D}') {
            if joiners && joins(out.chars().last()) && joins(chars.peek().copied()) {
                out.push(c);
            }
        } else if !c.is_control() && !is_spoofing(c) {
            out.push(c);
        }
    }
    out
}

/// As `clean_text`, on one line of at most `max` characters (names, reasons, labels). Names
/// keep no invisible characters at all.
pub fn clean_line(s: &str, max: usize) -> String {
    let flat: String = clean(s, false).chars().map(|c| if c == '\n' { ' ' } else { c }).collect();
    flat.trim().chars().take(max).collect::<String>().trim_end().to_string()
}

/// First non-empty line, at most `max` characters, cleaned for display.
pub fn preview(text: &str, max: usize) -> String {
    let text = clean_text(text);
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('…');
    }
    out
}

/// `nostr:npub1…` / `nostr:nprofile1…` mentions (NIP-21/27) shown as `@name`; other
/// `nostr:` links (notes, events) shortened.
pub fn mentions(text: &str, name: impl Fn(&crate::PublicKey) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("nostr:") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 6..];
        let end = tail.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(tail.len());
        let word = &tail[..end];
        let pk = if word.starts_with("npub1") || word.starts_with("nprofile1") { crate::nip19::parse_pubkey(word).ok() } else { None };
        match pk {
            Some(pk) => {
                out.push('@');
                out.push_str(&name(&pk));
            }
            None if word.len() > 16 => {
                out.push_str(&word[..word.len().min(12)]);
                out.push('…');
            }
            None => {
                out.push_str("nostr:");
                out.push_str(word);
            }
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// The first picture link in a message (http(s) URL ending in an image extension), if any.
pub fn picture_url(text: &str) -> Option<String> {
    text.split_whitespace().map(|w| w.trim_matches(|c: char| matches!(c, '(' | ')' | '<' | '>' | '"' | '\'' | ',' | '.'))).find_map(|w| {
        let path = w.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        let pic = [".jpg", ".jpeg", ".png", ".gif", ".webp"].iter().any(|e| path.ends_with(e));
        let url = crate::profile::https_url(w);
        (pic && !url.is_empty()).then_some(url)
    })
}

/// The first video link in a message (http(s) URL ending in a video extension), if any.
pub fn video_url(text: &str) -> Option<String> {
    text.split_whitespace().map(|w| w.trim_matches(|c: char| matches!(c, '(' | ')' | '<' | '>' | '"' | '\'' | ',' | '.'))).find_map(|w| {
        let path = w.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        let video = [".mp4", ".webm", ".mov", ".m4v"].iter().any(|e| path.ends_with(e));
        let url = crate::profile::https_url(w);
        (video && !url.is_empty()).then_some(url)
    })
}

/// The letter on an avatar.
pub fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "#".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        // 2026-10-30
        assert_eq!(civil(1_793_318_400 / 86_400), (2026, 10, 30));
    }

    #[test]
    fn relative_times() {
        let now = 1_793_318_400 + 12 * 3600;
        assert_eq!(ago(now, now), "now");
        assert_eq!(ago(now, now + 30), "now", "clock skew");
        assert_eq!(ago(now, now - 300), "5 min");
        assert_eq!(ago(now, now - 3 * 3600), "3 h");
        assert_eq!(ago(now, now - 2 * 86_400), "2 d");
        assert_eq!(ago(now, now - 20 * 86_400), "Oct 10");
        assert_eq!(ago(now, 1_700_000_000), "Nov 14 2023");
    }

    #[test]
    fn previews_and_initials() {
        assert_eq!(preview("\n  hello there  \nsecond", 70), "hello there");
        assert_eq!(preview("abcdef", 3), "abc…");
        assert_eq!(initial(" satoshi"), "S");
        assert_eq!(initial("ändi"), "Ä");
        assert_eq!(initial("--"), "#");
    }

    #[test]
    fn cleaning() {
        assert_eq!(clean_text("a\r\nb\rc\u{2028}d\u{0085}e"), "a\nb\nc\nd\ne");
        assert_eq!(clean_text("pay \u{202E}cba\u{200B}\u{2066}x\u{0007}\ty"), "pay cbax y");
        assert_eq!(clean_text("ä 日本 🙂 ❤\u{FE0F}"), "ä 日本 🙂 ❤\u{FE0F}");
        assert_eq!(clean_line("A\u{3164}\u{115F}\u{00AD}\u{2800}\u{206A}\u{E0041}\u{034F}B", 40), "AB");
        assert_eq!(clean_line("  Phone\nNEW MESSAGE\u{202E} ", 40), "Phone NEW MESSAGE");
        assert_eq!(clean_line("abcdef", 3), "abc");
        assert_eq!(preview("hi\u{2028}Alice: tap Publish now\u{202E}", 70), "hi");
        assert_eq!(preview("ok\rBob: second line", 70), "ok");
        assert_eq!(preview("\u{202E}\n\u{200B}\nreal", 70), "real");
    }

    #[test]
    fn mentions_and_pictures() {
        let pk = crate::SecretKey::generate().unwrap().public_key();
        let npub = crate::nip19::encode_npub(&pk);
        let t = format!("hi nostr:{npub}, see nostr:note1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq and nostr:x");
        assert_eq!(mentions(&t, |_| "Bob".into()), "hi @Bob, see note1qqqqqqq… and nostr:x");
        assert_eq!(picture_url("look https://x.example/a/b.JPG?w=2 nice").unwrap(), "https://x.example/a/b.JPG?w=2");
        assert!(picture_url("(http://localhost:7780/abc.png)").is_none(), "local links only when allowed (simulator)");
        crate::profile::allow_local_http(true);
        assert_eq!(picture_url("(http://localhost:7780/abc.png)").unwrap(), "http://localhost:7780/abc.png");
        assert!(picture_url("http://127.0.0.1.attacker.example/x.png").is_none());
        assert!(picture_url("https://x.example/page.html").is_none());
        assert!(picture_url("http://evil.example/a.png").is_none(), "plain http only for localhost");
    }
}
