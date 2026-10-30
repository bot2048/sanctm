# Sanctm: private Nostr DMs on Passport Prime

Private Nostr messages, written and read on Passport. The Passport holds the key and
seals, gift-wraps and opens every message itself (NIP-17). The phone is only the pipe: it
carries signed gift wraps between the Passport and Nostr relays and never sees a word,
a key, or who wrote to you.

It interoperates with any NIP-17 client (0xchat, Amethyst, Coracle, Damus, Nostur…).
Built for the Bitmela hackathon on Foundation's KeyOS SDK 1.1.0 (KeyOS 1.4).

<p align="center">
  <img src="docs/screenshots/chats.png" width="160" alt="Chats">
  <img src="docs/screenshots/chat.png" width="160" alt="A conversation">
  <img src="docs/screenshots/save.png" width="160" alt="Saving a picture">
  <img src="docs/screenshots/contact.png" width="160" alt="A contact's verified profile">
  <img src="docs/screenshots/chats-light.png" width="160" alt="Chats in the light theme">
</p>

**Contents:** [Why](#why) · [Features](#features) · [What each side sees](#what-each-side-sees) ·
[How it works](#how-it-works) · [The link, and QuantumLink](#the-link-and-quantumlink) ·
[Run it](#run-it) · [Tests](#tests) · [Layout](#layout) · [Limits](#limits-honest-list) ·
[Future scope](#future-scope) · [License](#license)

```
 Passport Prime (Sanctm app)          phone / laptop (companion)            Nostr relays
 ┌───────────────────────────────┐  link  ┌──────────────────────────┐  wss  ┌────────────┐
 │ nsec (from the app seed)      │◀──────▶│ subscribes kind 1059 #p  │◀─────▶│ damus, mom │
 │ write → rumor → seal → wrap   │ frames │ publishes wraps to the   │       │ primal,    │
 │ wrap → seal → rumor → read    │        │ recipient's 10050 relays │       │ snort …    │
 │ signs relay AUTH, 10050       │        │ sees: npub + ciphertext  │       └────────────┘
 └───────────────────────────────┘        └──────────────────────────┘
   link = QuantumLink on hardware (KeyOS 1.5 / QLv2), a WebSocket in the simulator today
```

## Why

A phone is a bad place for private conversations: keyboards, screenshots, cloud backups,
malware. Hardware wallets already keep a key offline and show what you sign on a screen you
can trust. Sanctm puts the two ends of a conversation, writing and reading, on that
screen. The internet-facing device becomes a dumb courier.

## Features

* **Private DMs on the device.** NIP-17 messages (NIP-44 v2 encryption, NIP-59 seal and gift
  wrap) are written, sealed, opened and stored on the Passport. Works with other Nostr DM apps.
* **Your identity, nothing new to back up.** The Nostr key is derived from the Passport's seed
  (NIP-06), or import an existing nsec by scanning its QR. Up to 10 accounts, each with its own
  sealed chats.
* **Real conversations.** Reactions, replies with quotes, deletes, disappearing messages
  (NIP-40), `nostr:` mentions, per-relay delivery status, an offline queue and resend.
* **Profiles you can trust.** Names, photos and NIP-05 addresses, with every profile's
  signature checked on the Passport; a verified address shows in place of the npub. Strangers
  are marked until you save them, and look-alike contact names are refused.
* **Pictures.** Encrypted photos (kind 15, as Amethyst and 0xchat send them) are decrypted on
  the Passport; picture links load only when you tap. **Save** keeps a picture sealed in
  Sanctm or writes a PNG copy to a folder you pick in KeyOS Files.
* **Relays done right.** Your DM inbox relays (kind 10050) and relay logins (NIP-42) are signed
  on the Passport, logins only for your own relays. 11 well-known relays by default.
* **Made for Passport.** Nostr purple theme that follows KeyOS light and dark, the Passport
  keyboard's Next and Done, a buzz and purple LED for new messages, and scanning someone's npub
  from the Launcher opens a chat with them.

## What each side sees

| | Passport | Companion (phone) | Relays |
|---|---|---|---|
| Your secret key | yes, derived from the app seed | never | never |
| Message text | yes | never | never |
| Who wrote to you | yes (verified) | never: wraps are signed by one-time keys | never |
| Whom you write to | yes | the recipient's pubkey (wrap `p` tag) | same |
| When | real time | jittered up to 2 days (NIP-59) | same |
| Contact names | yes, sealed on device | never | never |

## How it works

**Identity.** The KeyOS app seed (`GetAppSeed`, per app, from the device seed) is read as
BIP-39 entropy and the Nostr key is the NIP-06 path `m/44'/1237'/0'/0/0`. Nothing to back
up: restore the Passport and the same npub comes back. An existing `nsec` can be imported
instead (scan its QR); it is then kept sealed under the app seed.

**Your existing account.** Already on Nostr? On the Passport, choose "I already have a
Nostr account" (first screen) or Me → Accounts → "Use an existing nsec", then scan your nsec's
QR with the Passport camera or type it (`nsec1…` or 64 hex). The key is sealed under the app
seed and never shown again; the phone never sees it. Your npub, followers and NIP-17 history
come with it: past DMs arrive as the companion syncs. Each account keeps its own sealed
contacts and chats, so switching between it and the Passport-created key loses nothing.

**Sending.** The Passport builds the kind 14 rumor, seals it (kind 13, NIP-44, signed by
you), wraps it (kind 1059, NIP-44, signed by a fresh one-time key, timestamp jittered)
once for the recipient and once for yourself, and hands both wraps to the companion. The
companion looks up the recipient's kind 10050 DM relays and publishes; relay `OK`s come back
to the Passport and the message shows "on 3 relays". Written with no phone linked, it waits
sealed and goes out on the next link.

**Receiving.** The companion subscribes to kind 1059 events p-tagged to your npub and
forwards each wrap. The Passport checks the wrap signature, opens the wrap and the seal,
checks the seal signature, and refuses the message if the rumor's claimed author is not
the seal's signer (impersonation). Then: buzz, purple LED, toast.

**Relays.** "Publish as my DM relays" signs a kind 10050 on the Passport, listing exactly
the relays shown on screen, so other clients know where to deliver. Inbox relays that guard
gift wraps behind NIP-42 send an AUTH challenge; the Passport signs the kind 22242 answer
only for your own relays (once you have published a 10050, only the relays in it; never a
recipient's relay: that would tell it who sent the wrap), only for nonce-like challenges, and
at most twice per relay per minute, so the phone cannot use the Passport as a general login
signer.

**Profiles, pictures, reactions.** Sanctm speaks the rest of what other NIP-17 apps send
(details and a comparison with 0xchat, Amethyst, Coracle, Damus and Nostur in
[docs/FEATURES.md](docs/FEATURES.md)):

* *Profiles.* Names, photos and NIP-05 of the people you chat with, and your own. The phone
  fetches kind 0 events; the Passport checks each signature, so the phone cannot rename anyone.
  NIP-05 is shown as verified only if the phone resolved it to the same key. Start a chat by
  typing `alice@example.com`. Your contacts' and correspondents' profiles are fetched after a
  sync, a stranger's only when you open their chat, never all in one request. Profile
  fetching can be switched off (the phone learns whose profiles you look at).
* *Encrypted photos* (kind 15, as Amethyst, 0xchat and Nostur send them): the phone downloads
  only ciphertext; the Passport checks its hash, decrypts it (AES-256-GCM) and shows the picture.
  Picture links in messages load only when you tap them. Video and voice show as cards.
  **Save** on a picture offers two places: kept sealed in Sanctm (only this app opens it, and
  it shows without the phone), or a plain PNG copy in a folder you pick in KeyOS Files
  (Passport storage, the Airlock to copy it to a computer over USB, or a USB drive).
* *Reactions* (kind 7), *replies*, *deletes* (kind 5, author only), *disappearing messages*
  (NIP-40) and `nostr:` *mentions* shown as `@name`. Reactions and replies are sealed and
  gift-wrapped like messages.

**Untrusted input.** Everything from the phone, relays and other users is treated as hostile:
wrap and seal signatures are checked before a wrap id is remembered (a damaged copy cannot
shadow the real one), text is cleaned of bidi overrides and invisible characters before it
is shown, frames are size- and depth-limited, and the inbox has a byte budget (4 MB) in which
strangers, people who are not contacts and whom you never wrote to, are dropped first and
can never push out a contact's chat.

**At rest.** Profile, contacts and chats are AES-256-GCM files keyed from the app seed
(file name as associated data), named by a keyed tag rather than your npub. A copied disk
image shows nothing, not even whose account it holds. A file that exists but cannot be read
is never overwritten.

## The link, and QuantumLink

The goal: when the Passport is connected to your phone over QuantumLink, it is your
encrypted DM device. The app is built around that, with one caveat from the Bitmela
reference slide:
*"QuantumLink: Out of scope today · full QLv2 support arrives in KeyOS v1.5.0."* In SDK 1.1.0
/ KeyOS 1.4.0 the `os/quantum-link` messages are Envoy-specific (PSBTs, accounts, firmware)
and Foundation-signed; a third-party app can read link status but cannot send its own data.

So the transport is a seam (`src/transport.rs`):

* **Simulator:** a WebSocket server on port 9878 stands in for QuantumLink. The companion
  page connects to it. This is what the demo uses.
* **Passport on KeyOS 1.4:** no pipe. You can still write: messages are sealed and queued
  on the Passport, and your contacts and history work.
* **KeyOS 1.5+:** implement `Link::start` / `Link::send` over the QLv2 app channel. The
  frames (`core/src/link.rs`) and everything above them stay the same, and the companion's
  logic moves into the phone side of QuantumLink.

## Run it

Needs the Foundation SDK 1.1.0 (`foundation doctor` clean). Its Rust toolchain is only on the
path inside `foundation develop`.

**1. The app in the simulator**

```sh
git clone <this repo> sanctm && cd sanctm
foundation develop                     # a shell with the SDK toolchain
foundation sim                         # builds (first time: 5-15 min) and opens KeyOS with Sanctm
```

**2. The companion page (the "phone")**, in a second terminal:

```sh
cd companion && python3 -m http.server 8000
# open http://localhost:8000   (or from a phone on the same Wi-Fi: http://<laptop-ip>:8000)
```

It links to the app on `ws://<host>:9878` and uses 11 public relays by default
(eden.nostr.land, nostr.bitcoiner.social, nostr.mom, nostr.walletofsatoshi.com,
nostrelites.org, pyramid.fiatjaf.com, relay.damus.io, relay.mostr.pub, relay.primal.net,
relay.snort.social, wot.nostr.party). Change either under Settings, or with
`#ws=host:port&relays=wss://a,wss://b`. The page must be served over http (`file://` is refused).

**3. Someone to talk to.** Any NIP-17 app on the same relays: 0xchat, Amethyst, Coracle,
Flotilla, Nostur. Send a DM to the npub shown on the Passport (Me) and on the companion page.
Or use a second account on the same Passport (Me → Accounts → Create a new Passport key) and
message one account from the other.

**Accounts.** One Passport holds up to 10 Nostr accounts (Me → Accounts): keys derived from
its seed (NIP-06 accounts 0, 1, 2…) and imported nsecs, each with its own sealed chats,
contacts and profiles.

**Hardware.** `foundation build --release` builds and signs for a real Passport Prime
(`foundation cert gen` once). KeyOS 1.4 does not yet let third-party apps send data over
QuantumLink, so on hardware the app works offline: identity, npub QR, nsec import by camera,
writing (messages queue sealed), saved chats.

## Tests

`cd core && cargo test --release` (inside `foundation develop`, or with any recent Rust): the
protocol core's unit tests, including the official NIP-44 v2 vectors, NIP-06 and NIP-19
vectors, BIP-340 signatures, gift wrap round trips, impersonation and tampering, the inbox
budget, profiles, reactions, kind 15 file decryption and picture decoding.

Beyond these, the app was tested end to end against nostr-tools (an independent NIP-17
implementation) with a local NIP-42 relay and the real companion page in a headless browser,
fuzzed on every link frame, and reviewed screen by screen in dark and light.

## Layout

```
app-config.toml       KeyOS app: name, app-id (never change it: the Nostr key derives from it), permissions
src/                  KeyOS app: Slint glue, storage, haptics/LED, QR scan, Files, transport
ui/                   Slint screens (Nostr purple; follows the system light/dark theme)
resources/            icon, theme, UI icons
core/                 nip-ping-core: the protocol, pure Rust, no KeyOS deps
  src/keys.rs event.rs    secp256k1, NIP-01 events, BIP-340
  src/nip19.rs            npub / nsec / nprofile
  src/nip44.rs            NIP-44 v2 (official vectors in src/testdata)
  src/nip59.rs nip17.rs   seal, gift wrap, DMs, 10050, NIP-42 AUTH
  src/identity.rs         app seed -> NIP-06 key
  src/session.rs          the app's logic, UI-free
  src/link.rs             Passport <-> companion frames
  src/inbox.rs vault.rs   chats; sealed files
  src/profile.rs          kind 0 profiles, NIP-05
  src/media.rs            kind 15 files: decrypt and decode on the Passport; PNG copies
companion/            the phone/laptop page: link <-> relays, ciphertext only
docs/                 FEATURES.md (compared with other Nostr DM apps), DESIGN.md (UI spec), screenshots/
```

## Limits (honest list)

* No forward secrecy: NIP-17 uses static keys. A leaked key opens past messages.
* The companion can drop or delay messages (it cannot read or forge them).
* The recipient's pubkey is visible on relays (the wrap's `p` tag). That is NIP-17.
* One-to-one chats. Group messages from other clients are shown, marked "group chat", in
  the sender's thread; replying goes to the sender only.
* Simulator link: unauthenticated WebSocket on the LAN (port 9878). It refuses other
  websites (including sandboxed iframes and DNS rebinding) and frames over 128 KB; anyone on
  the LAN could still connect and see wraps and your npub. Serve the companion over http
  (`file://` is refused).
* A sealed file has no version counter: someone with write access to the storage could put
  back an older copy (it still has to be a genuine one from this Passport).
* Hardware: builds for the device, but there is no app link until QLv2.
* "Publish as my DM relays" lists every relay the companion uses (11 by default); NIP-17
  suggests a few that accept gift wraps. Edit the list under the companion's Settings first.
* Saving a copy to Files asks KeyOS for access once. The Airlock cannot be written while a
  computer is connected over USB, and USB drives may not be open to third-party apps yet.

## Future scope

* **QuantumLink.** When KeyOS 1.5 opens QLv2 to apps, implement `Link` over its app channel
  (`src/transport.rs`); the frames and everything above them stay the same, and the
  companion's job moves into the phone side of QuantumLink. Sanctm then works on hardware
  exactly as it does in the simulator.
* **Phone-free messaging.** Carry gift wraps as animated QR codes (UR), which the Passport
  camera already reads, for conversations with no network device at all.
* **Group chats.** NIP-17 rooms (a set of participants), with replies to the whole room.
* **Inbox relay picker.** Choose 1 to 3 DM relays for your kind 10050 (NIP-17 recommends a
  few), with hints about which relays accept gift wraps and protect reads with NIP-42.
* **Sending pictures and voice.** Capture with the Passport camera, encrypt on the device
  (kind 15) and let the phone upload only ciphertext.
* **Message requests.** Keep strangers in a separate inbox, with block lists and web-of-trust
  filters.
* **Forward secrecy.** Follow Nostr's MLS work (NIP-EE / Marmot) for ratcheted group and 1:1
  chats once it settles.
* **Sealed backup.** Export chats, contacts and kept pictures, sealed, to Passport storage or a
  USB drive, and restore them on a new Passport.
* **NFC.** Tap two Passports to exchange npubs (once KeyOS grants NDEF to third-party apps).
* **Languages.** Translate the UI with the SDK's localisation tooling.

## Credits

Builds on the Passport Prime Nostr Signer (NIP-44, keys, events; GPL-3.0-or-later). Reaction
pictures are Twemoji (CC-BY 4.0, see `resources/THIRD-PARTY.txt`). Publisher: Arpan Jain.

## License

GPL-3.0-or-later (see `LICENSE`), like Foundation's KeyOS SDK and apps.
