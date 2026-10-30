// SPDX-License-Identifier: GPL-3.0-or-later

//! Conversations, as stored (sealed) on the Passport.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::nip17::FileMeta;
use crate::Event;

pub const MAX_MESSAGES_PER_CHAT: usize = 400;
pub const MAX_CHATS: usize = 300;
/// Anyone can write to an npub, so chats nobody asked for are held on a short leash: a
/// stranger is a sender who is not a contact and whom we never wrote to. Their chats are
/// the first to go when room is needed, and there are at most this many.
pub const MAX_STRANGER_CHATS: usize = 50;
/// All chats together, counted as stored text plus a fixed overhead per message. Bounds
/// memory, the time a save takes and the size of the sealed file on the Passport.
pub const BYTE_BUDGET: usize = 4 * 1024 * 1024;
/// The message struct itself plus its JSON framing.
const MESSAGE_OVERHEAD: usize = std::mem::size_of::<Message>() + 96;
/// Gift wrap ids already opened (or refused), so a relay replaying them costs nothing.
pub const MAX_SEEN: usize = 3000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "s", rename_all = "snake_case")]
pub enum Status {
    /// Written while no phone was linked; goes out when one is.
    Queued,
    /// Handed to the companion, waiting for relays to answer.
    Sending,
    /// At least one relay stored it.
    Sent { relays: u32 },
    /// Every relay refused it (it stays queued and is retried on the next link).
    Failed { why: String },
    Received,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    /// Rumor id (hex).
    pub id: String,
    pub out: bool,
    pub text: String,
    pub at: u64,
    pub status: Status,
    /// Part of a group chat (more than one other participant); shown with a note.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub group: bool,
    /// Rumor id of the message this one answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// Reactions, one per person.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reactions: Vec<Reaction>,
    /// NIP-40: removed after this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
    /// A kind 15 file (encrypted on a server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<FileMeta>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reaction {
    /// Who reacted (hex).
    pub by: String,
    pub emoji: String,
    /// Rumor id of the kind 7, so a kind 5 that takes it back can remove it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Rumor time: of one person's reactions the newest wins, whatever order they sync in.
    #[serde(default)]
    pub at: u64,
}

/// Most reactions kept on one message.
pub const MAX_REACTIONS: usize = 12;
/// Reactions whose message has not arrived yet (sync order is random).
pub const MAX_ORPHANS: usize = 200;

impl Message {
    pub fn new(id: String, out: bool, text: String, at: u64, status: Status) -> Self {
        Message { id, out, text, at, status, group: false, reply_to: None, reactions: Vec::new(), expires: None, file: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Chat {
    /// The other participant's public key (hex).
    pub peer: String,
    pub messages: Vec<Message>,
    pub unread: u32,
}

impl Message {
    /// What this message counts against `BYTE_BUDGET`.
    pub fn cost(&self) -> usize {
        let why = match &self.status {
            Status::Failed { why } => why.len(),
            _ => 0,
        };
        let extra = self.reactions.len() * 160 +self.file.as_ref().map(|f| f.url.len() + 400).unwrap_or(0);
        self.text.len() + self.id.len() + why + extra + MESSAGE_OVERHEAD
    }
}

impl Chat {
    pub fn last(&self) -> Option<&Message> { self.messages.last() }
    pub fn last_at(&self) -> u64 { self.last().map(|m| m.at).unwrap_or(0) }
    pub fn cost(&self) -> usize { self.messages.iter().map(Message::cost).sum::<usize>() + self.peer.len() }

    /// A chat with a contact, with ourselves, or one we wrote in is never dropped to make
    /// room for strangers.
    fn protected(&self, keep: &Keep) -> bool {
        keep.me == Some(self.peer.as_str()) || keep.contacts.contains(&self.peer) || self.messages.iter().any(|m| m.out)
    }
}

/// Whose chats `Inbox::add_protected` keeps: ours (note to self) and our contacts' (hex keys).
#[derive(Default, Clone, Copy)]
pub struct Keep<'a> {
    pub me: Option<&'a str>,
    pub contacts: &'a [String],
}

/// A wrap waiting for a relay to accept it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Pending {
    pub wrap: Event,
    /// Whose inbox relays it goes to (hex).
    pub to: String,
    /// Rumor id this wrap carries.
    pub rumor: String,
    /// The recipient's copy (decides the message status); false for our own copy.
    pub primary: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Inbox {
    pub chats: Vec<Chat>,
    pub seen: VecDeque<String>,
    pub outbox: Vec<Pending>,
    /// Companion clock time of the last finished sync (0 = never).
    pub last_sync: u64,
    /// (message id, reaction) for messages not here yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<(String, Reaction)>,
    /// (message id, author) deletes, kept so a message that arrives after its delete (sync order
    /// is random) or comes back from a relay later stays deleted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tombstones: Vec<(String, String)>,
}

/// Deletes remembered.
pub const MAX_TOMBSTONES: usize = 1000;

impl Inbox {
    pub fn has_wrap(&self, id: &str) -> bool { self.seen.iter().any(|s| s == id) }

    pub fn note_wrap(&mut self, id: &str) {
        if !self.has_wrap(id) {
            self.seen.push_back(id.to_string());
            while self.seen.len() > MAX_SEEN {
                self.seen.pop_front();
            }
        }
    }

    pub fn chat(&self, peer: &str) -> Option<&Chat> { self.chats.iter().find(|c| c.peer == peer) }

    pub fn find_message(&self, rumor_id: &str) -> Option<&Message> {
        self.chats.iter().flat_map(|c| c.messages.iter()).find(|m| m.id == rumor_id)
    }

    /// Insert in time order. Returns false for a message already held (our own copy coming
    /// back from a relay), or one that did not fit (see `add_protected`).
    pub fn add(&mut self, peer: &str, msg: Message) -> bool { self.add_protected(peer, msg, Keep::default()) }

    /// As `add`, with the peers whose chats are protected (contacts, ourselves). Room is made
    /// by dropping, in order: the oldest messages of a chat past `MAX_MESSAGES_PER_CHAT`;
    /// strangers' chats past `MAX_STRANGER_CHATS`; then, past `MAX_CHATS` or `BYTE_BUDGET`,
    /// the stranger chats with the oldest last message, and only then the oldest messages of
    /// the biggest protected chat. (A stranger's full history is kept while there is room:
    /// right after importing an account, its correspondents are not contacts yet.) Returns
    /// whether `msg` is held afterwards.
    pub fn add_protected(&mut self, peer: &str, msg: Message, keep: Keep) -> bool {
        if self.find_message(&msg.id).is_some() {
            return false;
        }
        let author = if msg.out { keep.me.unwrap_or("") } else { peer };
        if self.tombstones.iter().any(|(id, by)| *id == msg.id && by == author) {
            return false;
        }
        let idx = match self.chats.iter().position(|c| c.peer == peer) {
            Some(i) => i,
            None => {
                self.chats.push(Chat { peer: peer.to_string(), messages: Vec::new(), unread: 0 });
                self.chats.len() - 1
            }
        };
        let (id, incoming) = (msg.id.clone(), !msg.out);
        let chat = &mut self.chats[idx];
        let pos = chat.messages.iter().rposition(|m| m.at <= msg.at).map(|p| p + 1).unwrap_or(0);
        chat.messages.insert(pos, msg);
        if chat.messages.len() > MAX_MESSAGES_PER_CHAT {
            let extra = chat.messages.len() - MAX_MESSAGES_PER_CHAT;
            chat.messages.drain(..extra);
            chat.unread = chat.unread.min(chat.messages.len() as u32);
        }
        self.make_room(&keep);
        if self.orphans.iter().any(|(t, _)| *t == id) {
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.orphans).into_iter().partition(|(t, _)| *t == id);
            self.orphans = rest;
            let me = keep.me.unwrap_or("");
            for (t, r) in mine {
                // Only the other person in this chat, or we ourselves, react to its messages.
                if r.by == peer || r.by == me {
                    self.react(&t, r);
                }
            }
        }
        match self.chats.iter_mut().find(|c| c.peer == peer) {
            Some(c) if c.messages.iter().any(|m| m.id == id) => {
                if incoming {
                    c.unread = (c.unread + 1).min(c.messages.len() as u32);
                }
                true
            }
            _ => false,
        }
    }

    /// Total against `BYTE_BUDGET`.
    pub fn cost(&self) -> usize { self.chats.iter().map(Chat::cost).sum() }

    fn oldest_stranger(&self, keep: &Keep) -> Option<usize> {
        self.chats.iter().enumerate().filter(|(_, c)| !c.protected(keep)).min_by_key(|(_, c)| c.last_at()).map(|(i, _)| i)
    }

    pub fn make_room(&mut self, keep: &Keep) {
        while self.chats.iter().filter(|c| !c.protected(keep)).count() > MAX_STRANGER_CHATS {
            let Some(i) = self.oldest_stranger(keep) else { break };
            self.chats.remove(i);
        }
        let mut total = self.cost();
        while self.chats.len() > MAX_CHATS || total > BYTE_BUDGET {
            if let Some(i) = self.oldest_stranger(keep) {
                total -= self.chats.remove(i).cost();
                continue;
            }
            // Only protected chats left. Past MAX_CHATS (500 contacts can outnumber it) the
            // one with the oldest last message goes, never the note-to-self chat; past the
            // budget, the oldest message of the biggest chat.
            if self.chats.len() > MAX_CHATS {
                let victim = self.chats.iter().enumerate().filter(|(_, c)| keep.me != Some(c.peer.as_str())).min_by_key(|(_, c)| c.last_at()).map(|(i, _)| i);
                match victim {
                    Some(i) => total -= self.chats.remove(i).cost(),
                    None => break,
                }
                continue;
            }
            let Some(big) = self.chats.iter_mut().filter(|c| !c.messages.is_empty()).max_by_key(|c| c.cost()) else { break };
            total -= big.messages.remove(0).cost();
            big.unread = big.unread.min(big.messages.len() as u32);
            if big.messages.is_empty() {
                let peer = big.peer.clone();
                self.chats.retain(|c| c.peer != peer || !c.messages.is_empty());
            }
        }
    }

    pub fn find_message_mut(&mut self, rumor_id: &str) -> Option<&mut Message> {
        self.chats.iter_mut().flat_map(|c| c.messages.iter_mut()).find(|m| m.id == rumor_id)
    }

    /// Add `by`'s reaction to message `target` (replacing theirs); kept aside until the
    /// message arrives if it is not here yet.
    pub fn react(&mut self, target: &str, r: Reaction) -> bool {
        // Taken back already (its kind 5 synced first), or older than the one we hold.
        if !r.id.is_empty() && self.tombstones.iter().any(|(id, by)| *id == r.id && *by == r.by) {
            return false;
        }
        match self.find_message_mut(target) {
            Some(m) => {
                if m.reactions.iter().any(|x| x.by == r.by && x.at > r.at) {
                    return true;
                }
                m.reactions.retain(|x| x.by != r.by);
                if m.reactions.len() < MAX_REACTIONS {
                    m.reactions.push(r);
                }
                true
            }
            None => {
                if self.orphans.iter().any(|(t, x)| t == target && x.by == r.by && x.at > r.at) {
                    return false;
                }
                self.orphans.retain(|(t, x)| !(t == target && x.by == r.by));
                self.orphans.push((target.to_string(), r));
                if self.orphans.len() > MAX_ORPHANS {
                    self.orphans.remove(0);
                }
                false
            }
        }
    }

    /// The chat holding message `id` (its peer).
    pub fn chat_of(&self, id: &str) -> Option<&str> {
        self.chats.iter().find(|c| c.messages.iter().any(|m| m.id == id)).map(|c| c.peer.as_str())
    }

    /// Remove messages `targets` written by `author` (hex) — or by us when `out`. Remembered,
    /// so they stay deleted if they arrive (again) later.
    pub fn delete_by(&mut self, targets: &[String], author: &str, out: bool) -> bool {
        for t in targets {
            if !self.tombstones.iter().any(|(id, by)| id == t && by == author) {
                self.tombstones.push((t.clone(), author.to_string()));
            }
        }
        if self.tombstones.len() > MAX_TOMBSTONES {
            let extra = self.tombstones.len() - MAX_TOMBSTONES;
            self.tombstones.drain(..extra);
        }
        let mut changed = false;
        for chat in &mut self.chats {
            let mine = chat.peer == author;
            let before = chat.messages.len();
            chat.messages.retain(|m| !(targets.contains(&m.id) && (if out { m.out } else { !m.out && mine })));
            if chat.messages.len() != before {
                changed = true;
                chat.unread = chat.unread.min(chat.messages.iter().filter(|m| !m.out).count() as u32);
            }
            // A kind 5 may also take back the author's reaction (kind 7) to a message.
            for m in &mut chat.messages {
                let n = m.reactions.len();
                m.reactions.retain(|r| !(r.by == author && targets.contains(&r.id)));
                changed |= m.reactions.len() != n;
            }
        }
        self.orphans.retain(|(_, r)| !(r.by == author && targets.contains(&r.id)));
        changed
    }

    /// Drop messages whose NIP-40 expiration has passed.
    pub fn purge_expired(&mut self, now: u64) -> bool {
        let mut changed = false;
        for chat in &mut self.chats {
            let before = chat.messages.len();
            chat.messages.retain(|m| m.expires.map(|e| e > now).unwrap_or(true));
            if chat.messages.len() != before {
                changed = true;
                chat.unread = chat.unread.min(chat.messages.len() as u32);
            }
        }
        changed
    }

    pub fn set_status(&mut self, rumor_id: &str, status: Status) -> bool {
        for chat in &mut self.chats {
            if let Some(m) = chat.messages.iter_mut().find(|m| m.id == rumor_id) {
                if m.status != status {
                    m.status = status;
                    return true;
                }
                return false;
            }
        }
        false
    }

    pub fn mark_read(&mut self, peer: &str) -> bool {
        match self.chats.iter_mut().find(|c| c.peer == peer) {
            Some(c) if c.unread > 0 => {
                c.unread = 0;
                true
            }
            _ => false,
        }
    }

    pub fn remove_chat(&mut self, peer: &str) {
        let gone: std::collections::HashSet<&str> =
            self.chats.iter().filter(|c| c.peer == peer).flat_map(|c| c.messages.iter().map(|m| m.id.as_str())).collect();
        let keep: Vec<bool> = self.outbox.iter().map(|p| p.to != peer && !gone.contains(p.rumor.as_str())).collect();
        let mut k = keep.into_iter();
        self.outbox.retain(|_| k.next().unwrap());
        self.chats.retain(|c| c.peer != peer);
    }

    /// Most recent first.
    pub fn ordered(&self) -> Vec<&Chat> {
        let mut v: Vec<&Chat> = self.chats.iter().filter(|c| !c.messages.is_empty()).collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.last_at()));
        v
    }

    pub fn unread_total(&self) -> u32 { self.chats.iter().map(|c| c.unread).sum() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, at: u64, out: bool) -> Message {
        Message::new(id.into(), out, id.into(), at, if out { Status::Queued } else { Status::Received })
    }

    #[test]
    fn ordering_dedup_unread() {
        let mut ib = Inbox::default();
        assert!(ib.add("bob", m("b", 20, false)));
        assert!(ib.add("bob", m("a", 10, true)));
        assert!(ib.add("bob", m("c", 30, false)));
        assert!(!ib.add("bob", m("c", 30, false)), "duplicate");
        let ids: Vec<_> = ib.chat("bob").unwrap().messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
        assert_eq!(ib.unread_total(), 2);
        assert!(ib.mark_read("bob"));
        assert!(!ib.mark_read("bob"));
        assert!(ib.add("carol", m("d", 5, false)));
        assert_eq!(ib.ordered().iter().map(|c| c.peer.as_str()).collect::<Vec<_>>(), ["bob", "carol"]);
        assert!(ib.set_status("a", Status::Sent { relays: 2 }));
        assert!(!ib.set_status("a", Status::Sent { relays: 2 }));
    }

    #[test]
    fn caps() {
        let mut ib = Inbox::default();
        let contacts = vec!["bob".to_string()];
        let keep = Keep { me: Some("me"), contacts: &contacts };
        for i in 0..(MAX_MESSAGES_PER_CHAT as u64 + 5) {
            ib.add_protected("bob", m(&format!("{i}"), i, false), keep);
        }
        assert_eq!(ib.chat("bob").unwrap().messages.len(), MAX_MESSAGES_PER_CHAT);
        assert_eq!(ib.chat("bob").unwrap().messages[0].id, "5");
        assert_eq!(ib.chat("bob").unwrap().unread as usize, MAX_MESSAGES_PER_CHAT);

        for i in 0..(MAX_SEEN + 3) {
            ib.note_wrap(&format!("w{i}"));
        }
        assert_eq!(ib.seen.len(), MAX_SEEN);
        assert!(!ib.has_wrap("w0"));
    }

    #[test]
    fn strangers_only_push_out_strangers() {
        let mut ib = Inbox::default();
        let contacts = vec!["alice".to_string()];
        let keep = Keep { me: Some("me"), contacts: &contacts };
        assert!(ib.add_protected("alice", m("a1", 1, false), keep));
        assert!(ib.add_protected("me", m("note", 2, true), keep));
        assert!(ib.add_protected("wrote-to", m("w1", 3, true), keep), "a chat we wrote in");
        for i in 0..(MAX_CHATS as u64 + 20) {
            ib.add_protected(&format!("s{i}"), m(&format!("s{i}"), 1000 + i, false), keep);
        }
        for p in ["alice", "me", "wrote-to"] {
            assert!(ib.chat(p).is_some(), "{p} was evicted");
        }
        let strangers = ib.chats.iter().filter(|c| c.peer.starts_with('s')).count();
        assert_eq!(strangers, MAX_STRANGER_CHATS);
        // The newest strangers stayed.
        assert!(ib.chat(&format!("s{}", MAX_CHATS + 19)).is_some());
        assert!(ib.chat("s0").is_none());
        // An old message from a new stranger does not displace newer ones.
        assert!(!ib.add_protected("late", m("late", 0, false), keep));
    }

    #[test]
    fn byte_budget_holds() {
        let mut ib = Inbox::default();
        let contacts: Vec<String> = (0..40).map(|i| format!("c{i}")).collect();
        let keep = Keep { me: Some("me"), contacts: &contacts };
        let big = "x".repeat(4000);
        let mut t = 0;
        // Contacts write far more than the budget holds.
        for round in 0..400 {
            for c in &contacts {
                t += 1;
                let mut msg = m(&format!("{c}-{round}"), t, false);
                msg.text = big.clone();
                ib.add_protected(c, msg, keep);
            }
            assert!(ib.cost() <= BYTE_BUDGET, "over budget: {}", ib.cost());
        }
        // Every contact still has a chat, and the newest message survived.
        assert_eq!(ib.chats.len(), contacts.len());
        assert!(ib.find_message("c39-399").is_some());
        assert!(ib.find_message("c0-0").is_none());
    }

    #[test]
    fn serde_roundtrip() {
        let mut ib = Inbox::default();
        ib.add("bob", m("a", 1, true));
        ib.set_status("a", Status::Failed { why: "blocked".into() });
        let json = serde_json::to_string(&ib).unwrap();
        assert_eq!(serde_json::from_str::<Inbox>(&json).unwrap(), ib);
    }
}
