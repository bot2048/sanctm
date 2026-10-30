# Sanctm features, checked against other Nostr DM apps

This was built from a code read of 0xchat, Amethyst (Quartz), Coracle/welshman/Flotilla, Damus
and Nostur, plus the current NIP-17 / 59 / 44 / 25 / 40 / 05 / 92 / B7 texts (October 2026).
✓ means done and tested in `core/` (`cargo test`). Spam handling (message requests, web of
trust, blocking) is deliberately left for a later version.

## Protocol (NIP-17 / NIP-59 / NIP-44)

| | Sanctm | Notes from other clients |
|---|---|---|
| Kind 14 chat messages, seal (13) and gift wrap (1059) with one-time keys, timestamps jittered up to 2 days into the past | ✓ | |
| Wrap signature checked before its id is remembered (a damaged copy cannot shadow the real one) | ✓ | 0xchat caches ids before checking |
| Seal signature checked; seal author must equal rumor author | ✓ | Amethyst overwrites, Damus overwrites, welshman skips the seal signature |
| Rumor id recomputed; mismatch refused, missing id computed | ✓ | 0xchat and welshman trust the claimed id |
| Seal tags tolerated (some clients put `expiration` there) | ✓ | |
| Copy to ourselves, dedup by rumor id, own echo ignored | ✓ | |
| Note to self (one wrap) | ✓ | |
| Sync window = last sync − 2 days (jitter) − 1 h, full history on first sync | ✓ | 0xchat: 3 days; Damus: negentropy |
| Kind 10050 DM relays: publish ours (exact list on screen), deliver to theirs | ✓ | |
| Recipient without 10050 | sends to our relays, like Damus, Nostur, 0xchat | Amethyst, Coracle and Flotilla refuse (spec MUST since June 2026) |
| NIP-42 AUTH signed on the Passport, only for our own relays (once published, only the relays in our 10050), nonce-like challenges, rate limited | ✓ | Amethyst: automatic only for own relays too |
| Messages ≤ 2000 bytes sent; NIP-44 64 KB payloads | ✓ | Damus and Nostur cap seals at 64 KB |
| Kind 15 encrypted files: AES-256-GCM, hex or base64 key/nonce, 12- or 16-byte nonces, old 0xchat nonce bug, `x` checked | ✓ decrypted on the Passport | Amethyst/0xchat 16-byte nonces, Coracle/Nostur 12 |
| Kind 7 reactions inside wraps (sent and shown, one per person and the newest wins, only from the people in that chat, symbols only: words become ★); changing ours sends a wrapped kind 5 for the old one, and their un-reacts are honoured | ✓ | Nostur, Amethyst, Flotilla, 0xchat |
| Kind 5 deletes inside wraps (author only; remembered, so a delete that arrives before its message still works) | ✓ | Amethyst, Flotilla; 0xchat publishes a public kind 5 (leak) |
| Replies: `e` tag with "reply" marker, else "root", else the last unmarked `e` (NIP-10), or `q`; never a "mention" | ✓ sent and shown | |
| NIP-40 expiration (disappearing messages) on the wrap, the seal or the rumor (earliest wins): never shown if expired, removed when due | ✓ | Nostur tags wrap + rumor, 0xchat rumor only |
| `subject` | shown as `[subject]` | |
| Group chats (participant set) | messages shown in the sender's chat, marked "group"; replies go 1:1 | rooms = set of author + `p` tags |
| Mentions do not add `p` tags (they would fork the room) | ✓ (we never add them) | |
| Unknown wrapped kinds (calls 25050, MLS 444, ephemeral 21059, private notes) | ignored safely | |
| Legacy NIP-04 (kind 4) | not supported, by choice | |

## Profiles and names

| | Sanctm |
|---|---|
| Kind 0 fetched by the phone, **signature checked on the Passport**, only for keys we asked for | ✓ |
| Privacy: our own profile asked for alone, others in small shuffled batches; contacts and people we wrote to fetched after sync, a stranger only when their chat is opened (a message alone tells the phone nothing) | ✓ |
| Name: your contact name, else `display_name`, else `name`, else short npub | ✓ |
| Profile picture shrunk to 64×64 by the phone, round avatar in chats, headers, contacts and accounts | ✓ |
| NIP-05 shown with ✓ only if the phone resolved it to the same key, and then shown instead of the npub under the name (chat header, contacts, accounts); `_@domain` shown as `domain`; lowercase; redirects ignored | ✓ |
| Start a chat from npub, nprofile, hex or `name@domain` | ✓ |
| `nostr:npub` / `nprofile` mentions shown as `@name` | ✓ |
| Switch off profile fetching (the phone learns whose profiles you look at) | ✓ |
| Several accounts on one Passport (up to 10, derived or imported), each with its own chats and profile; the phone follows a switch with fresh relay connections | ✓ |
| Contact names that look like an existing one (Cyrillic/Greek look-alikes, I/l/1, O/0) refused | ✓ |

## Media

| | Sanctm |
|---|---|
| Encrypted photo (kind 15): the phone downloads ciphertext only; decrypted, checked and decoded on the Passport (JPEG, PNG, WebP up to 12 MP; decoder memory capped at 64 MB) | ✓ |
| Encrypted GIF | shown as a card: open on a phone app | |
| Picture links in a message: loaded only when you tap (your IP stays private otherwise) | ✓ |
| Keep a picture on the Passport (sealed, up to 30) | ✓ |
| Video, voice, other files | shown as a card: open on a phone app (no player on the device) |
| Sending pictures from the Passport | no (no photo library on the device) |

## Safety and robustness

- Every string from others is cleaned for display (bidi overrides, invisible characters, line
  separators), names with such characters are refused. Joiners inside emoji and Indic/Persian
  words are kept in messages.
- A failed message says why in a few fixed words ("unreachable", "auth-required", …) and can be
  sent again with a tap; it also goes out when the phone links again.
- Link frames: 128 KB, 16 levels of JSON nesting, unknown types ignored; the phone cannot make
  the Passport store profiles or files it did not ask for.
- Inbox: 4 MB budget, strangers dropped first; pictures ≤ 360×480, files ≤ 8 MB, decoder memory
  bounded; a damaged picture is refused before decoding.
- What the phone says went wrong is shown only as a few fixed phrases; `http://` picture links
  only in the simulator (for the local test server); AUTH signatures capped per relay and in total.
- An independent review of drop 2 (memory, privacy, authorisation, spoofing) found 14 issues;
  all core and companion ones are fixed, with regression tests.
- A final QA round (app screens, protocol against other clients, security, companion page, and a
  new user following a step-by-step test script) found about 50 more issues; the fixes are in,
  with regression tests.
- Fuzzing of every frame type (in the development test suite, not in this repository).
