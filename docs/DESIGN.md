# Sanctm UI: design spec

## Goal and audience

A private DM app on a hardware wallet. The people who use it care about who can read their
messages, so the UI has to feel calm, exact and trustworthy: no noise, no surprises, every
security-relevant fact (who this is, whether it was sent, where it is stored) stated plainly.
Look: KeyOS's own warm neutrals (#231f20 in dark, white in light), following the system theme
live, with Nostr purple as the one accent. Montserrat for everything, Source Code Pro for keys.

Not a template: no gradients, no shadows, no cards around every block, no pills except the
unread count, no tinted circles behind icons, no tracked caps labels over every section. Group
with space, dividers and type instead. (Pass 5 in the review log says why.)

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

Never below 12 px. Device fonts: the KeyOS common set has Montserrat Light/Regular/Medium and
Source Code Pro Regular; Sanctm ships Montserrat SemiBold and Bold in `resources/fonts/`
(KeyOS registers an app's own fonts before the common ones), subset to the glyphs of the system
Regular so they cost about 196 KB each in RAM. So 600 is SemiBold, 700 and 800 are Bold. Hierarchy
still comes from size and colour too, never from weight alone.

## Radii, borders, elevation

* `r-sm` 4 (tags, QR card, quotes), `r-md` 6 (chips, reactions), `r-lg` 8 (buttons, inputs,
  panels, pressed rows, modal). Bubbles 10, with a 3 px corner on the sender's side for the
  tail and where bubbles of a run meet. Avatars, the unread count and the toggle stay round.
* Edge or fill, never both: panels and buttons are fills; an edge appears only when it says
  something (focus, an error, the negative button, an unknown-sender tag).
* No shadows and no gradients anywhere.
* Focus and selection: 2 px `S.accent` (inputs) or a 2 px `S.accent-text` halo (selected bubble).

## Colour tokens (ui/state.slint, `S.*`)

| Token | Role |
|---|---|
| `bg` | Page background: KeyOS #231f20 (dark) / #ffffff (light) |
| `panel` / `panel-hi` | Inputs, panels, pressed rows, incoming bubbles / secondary buttons, toast |
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
  centred, 22 / 700, elided; optional subtitle (mono only for keys). Right slot: one 40 px icon
  button (touch 56). The thread uses a conversation header instead: chevron, 40 px avatar,
  name and short npub left-aligned, contact button right. The chats page has its own: the name
  left (26 / 700), the account under it, then New chat and you (your avatar opens Me).
* **Buttons**: 56 px (`btn-h`), 48 px compact (`btn-h-sm`), radius 8, 17 / 700 labels.
  Primary = `fill`; secondary = `panel-hi`; negative = `bad` tint with a `bad` edge and label;
  tertiary = text only. Pressed: one step darker (90 ms). Disabled, every kind: `panel-hi`
  with a `faint` label and no touch.
* **Inputs** (`Field`): 14 / 600 label flush with the field, 56 px field, radius 8, `panel`
  fill, `line-hi` edge, 2 px `accent` when focused, `bad` with an error line under it. Secrets
  are masked with a Show / Hide toggle. Keys and addresses use mono.
* **List rows**: chats, contacts and accounts are plain rows, no card around them: a divider
  under each from the text column on. Chat rows are full width, 76 px, 48 px avatar at the
  gutter; unread: Bold name, `text` preview, purple time and count. Contact rows 64 px (40 px
  avatar), account rows 72 px (44 px avatar; the one in use says "In use" in `ok`, the others
  have a remove button with its own 56 px touch area). Pressed: `panel`.
* **Link line** (chats page): full width under the header: a dot and the state in the state's
  colour ("Phone linked", "Syncing", "No relays", "Offline"), the detail under it, a chevron;
  tapping it opens Me.
* **Sections** (`Section`): a divider, a sentence-case heading (14 / 600 `muted`), then the
  content on the page itself. Me, contact, accounts and onboarding are built from them.
* **Bubbles**: radius 10, 14 px padding (6 around a photo). In: `bubble-in`, left; out: `fill`,
  right; failed: 2 px `bad` outline and a red meta line. Max width = list width - 56. Messages
  from one side less than 5 minutes apart form a run (`run-start` from Rust): 3 px between
  bubbles of a run, 12 px between runs; on the sender's side the first bubble of a run has a
  round top corner and the others 3 px, and every bubble has a 3 px bottom corner as its tail.
  A line in `faint` above the first message says "Sealed on this Passport with NIP-17". Meta
  12 / 600 under the text, with a white double tick once a relay stored it. Reply quote: `r-sm`
  block with a 3 px accent bar. Reactions: 28 px tags hanging 8 px over the bubble's bottom
  edge, ringed in `bg`, ours tinted `accent-soft`.
* **Tags** (`Badge`): 20 px, radius 4, 1 px edge and 11 / 600 word in the tag's colour
  ("Unknown" in `warn` on chat rows: not in your contacts; "Imported" in `muted`).
  **Status** (`Pill`): an 8 px dot and the state in words, no box.
* **Avatars**: profile photo masked round in Rust (the software renderer does not clip), with
  a 1 px ring; fallback = initial on the key's tint ("?" for a bare npub). Sizes: 36 (chats
  header), 40 (thread header, contact rows), 44 (account rows), 48 (chat rows), 56 (new-chat
  result), 64 (Me), 72 (contact page).
* **Composer** (thread bottom): 52 px field (radius 8) growing to 4 lines, then scrolling to
  the cursor; 52 px send button (`fill`, or `panel` with a muted arrow when there is nothing to
  send). Above it, only when needed: the reply quote with an x, one small line for "no phone
  linked" or an error, the byte counter past 1800. The keyboard's accept key says Enter and
  starts a new line; tapping the conversation puts the keyboard away. The KeyOS keyboard is an
  overlay covering the bottom 306 px; the platform reports it in `Window.virtual-keyboard-size`, and the thread
  lifts the composer by that much.
* **Empty states**: a `t-title` heading and one sentence of `muted` body, centred; the calls to
  action are the page's bottom buttons.
* **Toasts**: on the chats page inline under the link line; elsewhere floating 16 px above the
  bottom bar, `panel-hi`, radius 8. 6 s.
* **Modal** (`ConfirmModal`): 432 px wide, `bg`, radius 8, 1 px `line-hi` edge, `overlay`
  scrim; title (in `bad` when it destroys something), body, confirm (primary, or negative)
  over Cancel. Every destructive action goes through it.
* **QR card**: white in both themes (scanners need the quiet zone), radius 4, code inset 5 px.

## Touch and accessibility

* Every touch target is at least 44 x 44 px (buttons 48-60, icon buttons 56 touch, reaction
  buttons 56, rows full height).
* Text contrast >= 4.5:1 in both themes (`faint` is the floor). Status is never colour alone:
  status lines carry words, failed bubbles carry a sentence.
* No motion that the software renderer pays for every frame: only short property animations
  (toggle knob, pressed fills, 150-200 ms).

## Pages and intent

* **Onboarding**: one promise, two choices. Left-aligned: logo, name, one line on what it is,
  "How it works" in three numbered lines (key from this Passport's seed; written, read and
  sealed here; the phone only carries gift wraps), then Create my Nostr account / I already
  have a Nostr key.
* **Import**: bring an nsec; the risk said once, in warning colour; the field is masked.
* **Chats**: who wrote, what is waiting, whether the phone is linked. Header (name, New chat,
  you), the link line, then the rows. Empty: someone new to Nostr has nobody to write to yet,
  so the first button is Show my npub (Me), the second Write to someone.
* **Thread**: the conversation and nothing else. Unknown senders get a warning band; tapping a
  message opens the action bar (reactions, reply, keep picture).
* **Compose**: one field, the byte budget, what happens on send (queued if no phone).
* **New chat**: scan, paste or look up a name@domain; see who it resolves to before starting.
* **Contact**: who this is (avatar and what their profile claims), then the task you came for:
  your name for them and Save, then Check their npub (chunked npub, how to compare it, QR),
  their self-declared profile, and Remove last. No Message button: back is the chat.
* **Me / Accounts**: you, then Your npub with its QR first (Show my npub lands here), then
  Accounts, Phone link, DM inbox relays, Settings, each a section.
* **Notice**: what happened, what to do, one primary action.
* **Picture viewer**: the picture, big, and Save. The Save sheet: Keep in Sanctm (sealed), Save
  to the last folder (one tap, once there is one), then a plain PNG copy in one of three places,
  each opening KeyOS's file browser on that place only to pick a folder: Passport storage, the
  Airlock (says to unplug from a computer first) and a USB drive. KeyOS's picker cannot start
  inside a folder, so the last folder is remembered per account (sealed profile) instead.

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

**Pass 4 (fonts, runs)**
* Bold was never bold: KeyOS has no Montserrat above Medium, so 600-800 fell back. Montserrat
  SemiBold and Bold now ship in `resources/fonts/` (subset to the system Regular's glyphs,
  about 196 KB each).
* Thread: bubbles group into runs with a tail corner.

**Pass 5 (not a template; flows walked as a new user)**
* The look read as machine-made. The tells listed in design write-ups of AI-generated UI
  (purple-tinted surfaces, purple gradients, 16-24 px radii on every card, cards around every
  block, pill badges and status chips, tracked caps labels over each section, icons on tinted
  circles, soft shadows) were all here. Now: KeyOS's neutrals, purple only where it means
  something, radius 8 (bubbles 10), plain rows and sections, no gradients or shadows.
* Name: Sanctm.
* Chats: the floating + became a New chat button in the header (it covered the last row);
  NEW on strangers read as "unread" and is now "Unknown"; the empty state leads with Show my
  npub, since a new user has to share their npub before anyone can write to them.
* Contact: naming them was below a 240 px QR; it now comes right after who they are. The
  Message button repeated Back and is gone. Rename is disabled until the name changes.
* Me: the npub and QR come right after you, above Accounts.
* Thread: the unknown-sender action says Add contact (not Save, which suggested an instant
  save); the empty conversation says how to react or reply (tap a message).
* Onboarding: one explanation instead of a diagram and two promises that said the same thing.
* Screens are rendered without the simulator by `tools/ui-shots/shots.py` (local, in the
  ignored `tools/`): every page with sample data, dark and light, through
  `foundation-slint-viewer --screenshot`.
