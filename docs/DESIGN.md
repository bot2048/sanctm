# Sanctum UI: design spec

## Goal and audience

A private DM app on a hardware wallet. The people who use it care about who can read their
messages, so the UI has to feel calm, exact and trustworthy: no noise, no surprises, every
security-relevant fact (who this is, whether it was sent, where it is stored) stated plainly.
Look: Nostr purple on near-black (dark) or on lavender-white (light), following the KeyOS
system theme live. Montserrat for everything, Source Code Pro for keys.

## Canvas

* 480 x 800 px, touch only. The KeyOS status bar covers the top 40 px (`S.top-inset`).
* Page padding (`S.gutter`) 24 px, Foundation's native page padding. Chat bubbles use a tighter
  14 px list inset (`S.list-inset`) so text gets the width.
* Bottom action areas sit on a 1 px divider, 12 px above and 16 px below their buttons.

## Spacing scale (4 px base)

`S.s1` 4 · `S.s2` 8 · `S.s3` 12 · `S.s4` 16 · `S.s5` 20 · `S.s6` 24 · `S.s8` 32.
Between sections of a page: 16 (`s4`). Inside a card: 16 padding, 10-12 between lines. Between
list rows: 8 (contacts, accounts), 12 on the chats page (rows carry more). Label to field: 8.

## Type scale (Montserrat unless noted)

| Token | Size / weight | Use |
|---|---|---|
| `t-display` | 40 / 800, tracking 8 | Onboarding wordmark only |
| `t-title-lg` | 26 / 800 | Empty-state and notice headings |
| `t-title` | 22 / 700 | Page header titles (centred) |
| `t-heading` | 19 / 700 | Thread header name, profile names, card titles |
| `t-row` | 17 / 700 | List row titles |
| `t-body` | 16 / 400 | Paragraphs; bubble text is 17 |
| `t-small` | 14 / 400-600 | Row details, field labels, helper text |
| `t-meta` | 12 / 600 | Times, bubble meta, eyebrows (eyebrows 12 / 700, tracking 1.4) |
| `t-button` | 18 / 700 | Button labels (Foundation uses 20; our labels are longer) |
| mono | Source Code Pro 15-16 (chunked npub), 12 (short npub) | Keys only |

Never below 12 px. Device fonts: the SDK provides Montserrat and Source Code Pro (the KeyOS
common set has Montserrat Light/Regular/Medium and Source Code Pro Regular), so 700 / 800 may
render as the nearest weight on hardware: hierarchy must also come from size and colour, never
from weight alone. (`resources/fonts/` is not used: the Foundation CLI materializes it as a link
to the SDK's shared fonts.)

## Radii, borders, elevation

* `r-sm` 8 (badges), `r-md` 12 (cards inside bubbles, quotes), `r-lg` 16 (panels, list rows),
  `r-xl` 24 (inputs, modal, QR card), bubbles 20, buttons and chips are pills (height / 2).
* Flat by default: 1 px `S.line` borders. Emphasis is a coloured border, not a shadow
  (`S.accent` at 55 % for unread rows, `S.ok` for a live link, `S.bad` for errors).
* Shadows only on things that float: the + button, the toast, the modal.
* Focus and selection: 2 px `S.accent` (inputs) or a 2 px `S.accent-text` halo (selected bubble).

## Colour tokens (ui/state.slint, `S.*`)

| Token | Role |
|---|---|
| `bg` | Page background |
| `panel` / `panel-hi` | Cards, rows, inputs / pressed rows, secondary buttons, quotes |
| `line` / `line-hi` | Hairlines and card borders / input and button borders |
| `text` / `muted` / `faint` | Primary text / secondary text / times, hints (all >= 4.5:1 on bg and panel) |
| `accent` / `accent-text` / `accent-soft` | Brand strokes / purple text / tinted fills |
| `fill` (#8e30eb) / `on-accent` | Behind white labels: primary buttons, out bubbles, badges |
| `bubble-in` / `bubble-in-line` / `bubble-out` | Message bubbles |
| `ok` / `warn` / `bad` | Linked, verified / caution, unknown sender / refused, destructive |
| `overlay` | Modal scrim (#111111e6, Foundation's) |

Text on colour: white on `fill` (5.6:1); on out bubbles meta and quotes are white at 90 % and
cards inside them darken the purple (#0000002e) so every line stays above 4.5:1. Purple text uses `accent-text`, never `accent`.

## Components

* **Header** (64 px): 32 px chevron at 16 px from the edge with a 64 px touch area; title
  centred, 22 / 700, elided; optional mono subtitle. Right slot: one 40 px round icon button
  (touch 56). The thread uses a conversation header instead: chevron, 40 px avatar, name and
  short npub left-aligned, info button right.
* **Buttons**: pills, 60 px (`btn-h`), 48 px compact (`btn-h-sm`) inside cards. Primary = `fill`;
  secondary = `panel-hi` + `line-hi` border; negative = `bad` tint + `bad` label; tertiary =
  text only. Pressed: one step darker (90 ms). Disabled, every kind: `panel-hi` with a `faint`
  label and no touch (not a faded purple: white on pale lavender is unreadable in light).
* **Inputs** (`Field`): 14 / 600 label 16 px in, 56 px field, radius 24, `panel` fill,
  `line-hi` border, 2 px `accent` when focused, `bad` with an error line under it. Secrets are
  masked with a Show / Hide toggle. Keys and addresses use mono.
* **List rows**: `panel`, radius 16, 1 px `line`; chat rows 80 px with a 48 px avatar, contact rows
  68 px with a 40 px avatar, account rows 72 px with a 44 px avatar (the one in use: 2 px
  accent border and an IN USE pill). Whole row is the touch target; row actions are icon
  buttons with their own 56 px touch area.
* **Bubbles**: radius 20, 14 px padding (6 around a photo). In: `bubble-in` + border, left;
  out: `fill`, right; failed: 2 px `bad` outline and a red meta line. Max width = list width
  - 56. Meta 12 / 600 under the text, right-aligned on out bubbles, with a white double tick
  once a relay stored it. Reply quote: `r-md` block
  with a 3 px accent bar. Reactions: 28 px pills hanging 8 px over the bubble's bottom edge,
  ringed in `bg`, ours tinted `accent-soft`.
* **Chips and badges**: chips are 40 px pills (touchable), status pills 28 px, badges 20 px
  (`r-sm`, 10 / 800 caps).
* **Cards** (`Panel`): `panel`, radius 16, 16 padding, eyebrow title in caps.
* **Avatars**: profile photo masked round in Rust (the software renderer does not clip), with
  a 1 px ring; fallback = initial on the key's tint ("?" for a bare npub). Sizes: 40 (thread
  header, contact rows), 44 (account rows), 48 (chat rows), 56 (new-chat result), 72 (Me),
  96 (contact page).
* **Composer** (thread bottom): 52 px pill field growing to 4 lines, then scrolling to the
  cursor; 52 px round send button (`fill`, or `panel-hi` with a muted arrow when there is
  nothing to send). Above it, only when needed: the reply quote with an x, one small line for
  "no phone linked" or an error, the byte counter past 1800. The KeyOS keyboard is an overlay
  covering the bottom 306 px (its 90 px top bar is see-through); the platform reports it in
  `Window.virtual-keyboard-size`, and the thread lifts the composer by that much.
* **Empty states**: 96 px circled icon, `t-title-lg` heading, one sentence of `muted` body,
  the call to action as the page's bottom button.
* **Toasts**: on the chats page inline under the status card; elsewhere floating 16 px above the
  bottom bar, `panel` with a purple border and shadow. 6 s.
* **Modal** (`ConfirmModal`): 424 px wide, radius 24, `overlay` scrim, 48 px icon, title,
  body, Cancel (secondary) + confirm (primary, or negative when it destroys something).
  Every destructive action goes through it; forgetting an imported nsec says it cannot be
  recovered from this Passport.
* **QR card**: white in both themes (scanners need the quiet zone), radius 24, code inset 5 px.

## Touch and accessibility

* Every touch target is at least 44 x 44 px (buttons 48-60, icon buttons 56 touch, reaction
  buttons 56, rows full height).
* Text contrast >= 4.5:1 in both themes (`faint` is the floor). Status is never colour alone:
  pills carry words, failed bubbles carry a sentence.
* No motion that the software renderer pays for every frame: only short property animations
  (toggle knob, pressed fills, 150-200 ms).

## Pages and intent

* **Onboarding**: one promise, two choices. Logo, wordmark, the two facts that matter (key on
  Passport, phone carries envelopes), Create / I already have an account.
* **Import**: bring an nsec; the risk said once, in warning colour; the field is masked.
* **Chats**: who wrote, what is waiting, whether the phone is linked. Status card first,
  then rows; the + button starts a chat.
* **Thread**: the conversation and nothing else. Unknown senders get a warning band; tapping a
  message opens the action bar (reactions, reply, keep picture).
* **Compose**: one field, the byte budget, what happens on send (queued if no phone).
* **New chat**: scan, paste or look up a name@domain; see who it resolves to before starting.
* **Contact**: who this is (their profile, clearly marked as self-declared), the npub to verify,
  your name for them, destructive actions last.
* **Me / Accounts**: your profile and npub to share; accounts to switch, add or remove; link,
  relays, settings.
* **Notice**: what happened, what to do, one primary action.
* **Picture viewer**: the picture, big, and Save: kept sealed in Sanctum, or a PNG copy in a
  folder picked in KeyOS Files.

## Review log

Every screen was rendered in dark and light and reviewed one page at a time for spacing, type,
alignment, hierarchy, colour balance and touch size.

**Global (pass 1)**
* Ad-hoc numbers replaced by tokens: spacing scale, type scale, radii, heights (`S.*`).
* Page padding 20 -> 24 (Foundation). Buttons: 56 px rounded rectangles on the SDK Button ->
  our own 60 px pills with pressed and readable disabled states (the SDK Button stub had
  neither, and its disabled look was a faded purple).
* Headers: left title 21 px after a 56 px round back button -> centred 22 / 700 title,
  chevron with a 64 px touch strip, hairline under it.
* Inputs: SDK Input (label in caps inside the component) -> `Field` with Foundation's
  numbers, purple focus ring, error line, masking for secrets.
* Every "tap again to ..." confirmation -> `ConfirmModal`; forgetting an imported key is
  critical (red) and says it cannot be recovered from this Passport.
* Bubble meta was 11 px, under the 12 px floor -> 12 px.
* Bold weights: the KeyOS common fonts stop at Montserrat Medium, so every heading also
  differs from body text in size (22 / 19 / 17 vs 16) and every secondary line in colour.

**Per page (pass 2)**
* Onboarding: actions pinned at the bottom as a button section; wordmark 44 -> 40 px with
  tighter tracking so it does not crowd the logo; the two promises get 16 px between them.
* Import: secret field masked with Show / Hide; "Import and seal it" turns primary once there
  is text; the title says "Add an account" when one exists, and back returns to Accounts.
* Chats: the simulator-only persona chip is gone; your avatar replaces the generic person
  icon; with several accounts the subtitle names the one in use; NEW badge sized to its text
  (was a fixed 40 px box); empty state = circled icon, heading, sentence, "Start a chat"
  bottom button (the + button would have duplicated it).
* Thread: conversation header with their avatar; contact details is a person icon (was a
  bare "i" in Montserrat); unknown-sender band copy shortened (it wrapped one word); empty
  thread centred instead of sinking to the bottom; message tap shows a 2 px halo and the
  action bar (5 reactions as pictures, Reply, Keep picture) above the composer; header right
  button becomes x while a message is selected.
* Compose page -> inline composer (requested): stays on the thread, keeps focus after send,
  follows the newest message unless you scrolled up, lifts above the keyboard; tapping the
  conversation puts the keyboard away.
* New chat: Start chat is the primary action (Scan is secondary); labels in sentence case;
  the result card shows who the field resolves to before anything is saved; name optional
  and prefilled from their profile, and cleared again when the field changes to someone else
  (found in review: a lookup error kept the previous person's name).
* Contact: who they are first (96 px avatar, profile name, @handle, NIP-05 with the phone's
  verdict), profile text in a card marked as self-declared, then the QR inside the npub card
  (240 px), your name for them (prefilled from the profile when unsaved), destructive actions
  last with extra space before them.
* Me: renamed from "Your Nostr identity" to match the button that opens it; profile card
  first, Accounts row, QR (320 px) in the npub card, link, relays, Settings toggle.
* Accounts (new): rows with avatar, IMPORTED badge, IN USE pill; remove is an x with its own
  touch area and a modal; add-account choices in one card with what each key means.
* Notice: icon, title and text centred in the space above a bottom button section; text in
  `muted` so the title leads.
* Picture viewer (new): fit to the screen (at most 1.25x), Keep / Forget at the bottom; the
  caption under it lost an icon that wrapped onto its own line.
* Arrival toasts name only contacts; anyone else is "npub1… (not a contact)", since a
  stranger's profile can call itself "Note to self" or borrow a contact's name.
* Light theme: disabled primary buttons were unreadable (white on pale lavender) -> new
  disabled style; the send button's disabled state uses a muted arrow instead of a 70 %
  white one.

**Micro pass (pass 3)**
* Header title baselines: one height (64) and one title style on every page; subtitles mono
  only for keys ("2 on this Passport" is sans).
* List rhythm: chats 12 px apart, contacts and accounts 8; avatars on one left edge (16 px).
* Bubbles: radius 18 -> 20, quote block 12 px radius with an inset 3 px bar that does not
  poke out of the rounded corners, reaction pills ringed in the page colour where they
  overlap the bubble edge; photo-only bubbles use a 6 px frame instead of 14.
* Sent state: a double tick after the relay count (no animation: the message list is rebuilt
  on every change, so a transition would replay; not worth the software renderer's time).
* Pressed states on every touchable surface (rows, cards, chips, icon buttons, reactions,
  send); toggle knob slides in 160 ms; the media card icon pulses while a file is opening.
* No page transitions: a cross-fade means layered opacity on a full screen in the software
  renderer, for little gain.
* Touch sizes checked: header back 64 x 64, icon buttons 56, chips 44, reactions and send 56 /
  52, media cards 60+, field Show / Hide 72 x 56.

