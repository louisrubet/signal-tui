use chrono::NaiveDateTime;

pub struct Message {
    /// Signal message id: the sender's timestamp in milliseconds (what quotes refer to).
    pub id: u64,
    /// ACI of the author, needed to quote the message.
    pub author: Option<String>,
    pub from_me: bool,
    pub sender_name: String,
    pub text: String,
    pub timestamp: NaiveDateTime,
    /// Index, in the same discussion, of the message this one replies to.
    pub reply_to: Option<usize>,
    pub forwarded: bool,
    /// Emoji reactions, one per author.
    pub reactions: Vec<Reaction>,
    /// Deleted for everyone: shown as "This message was deleted".
    pub deleted: bool,
}

/// How long after sending a message can be deleted for everyone (Signal's default limit).
pub const DELETE_MAX_AGE_MS: u64 = 24 * 60 * 60 * 1000;

/// An emoji reaction to a message.
#[derive(Debug, Clone, PartialEq)]
pub struct Reaction {
    /// ACI of who reacted.
    pub author: String,
    pub emoji: String,
    /// Our own reaction (from this device or another).
    pub mine: bool,
}

impl Message {
    /// A message written here, sent now.
    pub fn mine(id: u64, text: String, reply_to: Option<usize>, forwarded: bool) -> Self {
        Message {
            id,
            author: None,
            from_me: true,
            sender_name: String::new(),
            text,
            timestamp: chrono::Local::now().naive_local(),
            reply_to,
            forwarded,
            reactions: Vec::new(),
            deleted: false,
        }
    }

    /// Whether we can delete it for everyone at `now_ms`: ours, sent less than 24 h ago.
    pub fn deletable(&self, now_ms: u64) -> bool {
        self.from_me && !self.deleted && now_ms.saturating_sub(self.id) < DELETE_MAX_AGE_MS
    }

    /// Deleted for everyone: its text and reactions go away.
    pub fn mark_deleted(&mut self) {
        self.deleted = true;
        self.text.clear();
        self.reactions.clear();
    }

    /// Sets the reaction of `author` (replacing theirs), or removes it with `None`.
    pub fn set_reaction(&mut self, author: &str, mine: bool, emoji: Option<&str>) {
        self.reactions.retain(|r| r.author != author);
        if let Some(emoji) = emoji {
            self.reactions.push(Reaction { author: author.to_string(), emoji: emoji.to_string(), mine });
        }
    }

    /// Our own reaction, if any.
    pub fn my_reaction(&self) -> Option<&str> {
        self.reactions.iter().find(|r| r.mine).map(|r| r.emoji.as_str())
    }

    /// The reactions grouped by emoji, in order of appearance: `❤️ 2  👍` (count when > 1).
    pub fn reaction_summary(&self) -> String {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        for reaction in &self.reactions {
            match counts.iter_mut().find(|(emoji, _)| *emoji == reaction.emoji) {
                Some((_, count)) => *count += 1,
                None => counts.push((&reaction.emoji, 1)),
            }
        }
        let parts: Vec<String> = counts
            .into_iter()
            .map(|(emoji, count)| if count > 1 { format!("{emoji} {count}") } else { emoji.to_string() })
            .collect();
        parts.join("  ")
    }
}

pub struct Discussion {
    pub title: String,
    pub messages: Vec<Message>,
    /// Number of messages received since it was last opened.
    pub unread: usize,
    /// Rank in the "Pinned" list (pin order: a chat pinned later comes after); `None` when
    /// not pinned.
    pub pinned: Option<u32>,
}

impl Discussion {
    /// Index of the oldest unread message: the `unread`-th received one from the end.
    pub fn first_unread(&self) -> Option<usize> {
        if self.unread == 0 {
            return None;
        }
        let mut left = self.unread;
        for (i, m) in self.messages.iter().enumerate().rev() {
            if !m.from_me {
                left -= 1;
                if left == 0 {
                    return Some(i);
                }
            }
        }
        None
    }

    pub fn new(title: String) -> Self {
        Discussion { title, messages: Vec::new(), unread: 0, pinned: None }
    }

    /// Index of the message with this Signal id.
    pub fn position_of(&self, id: u64) -> Option<usize> {
        self.messages.iter().rposition(|m| m.id == id)
    }
}

/// Returns the http(s) links found in `text`, without trailing punctuation.
pub fn links(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .map(|word| word.trim_start_matches(['(', '<', '"', '\'']))
        .filter(|word| word.starts_with("http://") || word.starts_with("https://"))
        .map(|word| word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', '>', '"', '\'']))
        .collect()
}

fn dt(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
}

fn msg(from_me: bool, sender_name: &str, text: &str, ts: &str) -> Message {
    let timestamp = dt(ts);
    Message {
        id: timestamp.and_utc().timestamp_millis() as u64,
        author: None,
        from_me,
        sender_name: sender_name.to_string(),
        text: text.to_string(),
        timestamp,
        reply_to: None,
        forwarded: false,
        reactions: Vec::new(),
        deleted: false,
    }
}

pub fn mock_discussions() -> Vec<Discussion> {
    vec![
        Discussion {
            title: "Alice Martin".to_string(),
            unread: 0,
            pinned: Some(0),
            messages: vec![
                msg(false, "Alice Martin", "Hey! Are we still on for tomorrow?", "2026-07-15 09:12"),
                msg(true, "", "Yes, absolutely. What time works for you?", "2026-07-15 09:15"),
                msg(false, "Alice Martin", "How about 10am at the usual place?", "2026-07-15 09:16"),
                msg(true, "", "Works for me.", "2026-07-15 09:17"),
                msg(false, "Alice Martin", "Great, see you then! Also, do you remember the name of that restaurant we went to last month with the really long menu and the amazing dessert selection?", "2026-07-16 18:40"),
                msg(true, "", "Ah yes, it was called \"La Petite Table\", I think.", "2026-07-16 18:45"),
                msg(false, "Alice Martin", "That's the one, thanks!", "2026-07-16 18:46"),
                msg(true, "", "Morning!", "2026-07-17 08:02"),
            ],
        },
        Discussion {
            title: "Family Group".to_string(),
            unread: 2,
            pinned: None,
            messages: vec![
                msg(false, "Mom", "Don't forget dinner on Sunday.", "2026-07-13 12:00"),
                msg(false, "Dad", "I'll bring the wine.", "2026-07-13 12:03"),
                msg(true, "", "Sounds good, I'll bring dessert.", "2026-07-13 12:05"),
                msg(false, "Mom", "Perfect, see you all then.", "2026-07-14 20:10"),
            ],
        },
        Discussion {
            title: "Bob Dupont".to_string(),
            unread: 0,
            pinned: None,
            messages: vec![
                msg(false, "Bob Dupont", "Did you push the fix?", "2026-07-17 10:00"),
                msg(true, "", "Yep, just pushed it.", "2026-07-17 10:01"),
                msg(false, "Bob Dupont", "Nice, testing now.", "2026-07-17 10:02"),
                msg(
                    false,
                    "Bob Dupont",
                    "Found the root cause: https://github.com/whisperfish/presage/issues and the docs (https://docs.rs/presage).",
                    "2026-07-17 10:30",
                ),
                Message { reply_to: Some(3), ..msg(true, "", "Thanks, having a look.", "2026-07-17 10:35") },
            ]
            .into_iter()
            .enumerate()
            .map(|(i, mut m)| {
                // A few reactions for the demo.
                if i == 3 {
                    m.set_reaction("me", true, Some("\u{1f44d}"));
                    m.set_reaction("bob", false, Some("\u{1f44d}"));
                }
                if i == 4 {
                    m.set_reaction("bob", false, Some("\u{2764}\u{fe0f}"));
                }
                m
            })
            .collect(),
        },
        Discussion {
            title: "Chloe Renard".to_string(),
            unread: 0,
            pinned: None,
            messages: vec![
                msg(false, "Chloe Renard", "Happy birthday!! 🎉", "2026-06-01 07:30"),
                msg(true, "", "Thank you so much!", "2026-06-01 08:15"),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::links;

    #[test]
    fn extracts_links() {
        assert_eq!(
            links("See https://a.example/x, and (http://b.example/y). Not ftp://c.example"),
            vec!["https://a.example/x", "http://b.example/y"]
        );
        assert!(links("no link here").is_empty());
    }
}
