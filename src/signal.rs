//! Connection to the Signal servers through presage, as a linked (secondary) device.

// `SignalError` is presage's error type, large but not ours to box.
#![allow(clippy::result_large_err)]

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::pin::pin;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};
use futures::channel::oneshot;
use futures::{StreamExt, future};
use presage::Manager;
use presage::libsignal_service::configuration::SignalServers;
use presage::libsignal_service::content::{Content, ContentBody};
use presage::libsignal_service::protocol::ServiceId;
use presage::manager::Registered;
use presage::model::identity::OnNewIdentity;
use presage::model::messages::Received;
use presage::libsignal_service::prelude::Uuid;
use presage::proto::data_message::Quote;
use presage::proto::sync_message::{Content as SyncContent, Sent};
use presage::proto::{DataMessage, GroupContextV2, SyncMessage};
use presage::store::ContentsStore;
pub use presage::store::Thread;
use presage_store_sqlite::{SqliteStore, SqliteStoreError};
use tokio::time::Instant;
use url::Url;

use crate::data::{Discussion, Message};

pub type SignalError = presage::Error<SqliteStoreError>;
pub type SignalManager = Manager<SqliteStore, Registered>;

/// Where the store lives: `$SIGNAL_TUI_DB`, or `signal-tui/signal-tui.db3` in the XDG data
/// directory (`~/.local/share` by default). Creates the parent directory, private to the user.
pub fn default_store_path() -> std::io::Result<PathBuf> {
    let path = match std::env::var_os("SIGNAL_TUI_DB") {
        Some(path) => PathBuf::from(path),
        None => {
            let data_dir = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
                .ok_or_else(|| std::io::Error::other("neither XDG_DATA_HOME nor HOME is set"))?;
            data_dir.join("signal-tui").join("signal-tui.db3")
        }
    };
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        // The store holds the device keys: keep the directory private.
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(parent)?;
    }
    Ok(path)
}

/// Opens (creating it if needed) the local SQLCipher store holding the device keys.
pub async fn open_store(path: &str, passphrase: Option<&str>) -> Result<SqliteStore, SqliteStoreError> {
    SqliteStore::open_with_passphrase(path, passphrase, OnNewIdentity::Trust).await
}

/// Deletes the local store (database plus its SQLite `-wal`/`-shm` files), forgetting the link.
///
/// The device stays listed on the phone (Settings > Linked devices) until it is removed there.
pub fn remove_store(path: &str) -> std::io::Result<()> {
    for file in [path.to_string(), format!("{path}-wal"), format!("{path}-shm")] {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

/// Links this client as a secondary device of an existing Signal account.
///
/// `provisioning_link` receives the `sgnl://linkdevice?...` URL as soon as the Signal
/// servers hand it out; it must be shown to the user (usually as a QR code) and scanned
/// from the primary phone. The returned future resolves once the phone confirms.
pub async fn link_device(
    store: SqliteStore,
    device_name: &str,
    provisioning_link: oneshot::Sender<Url>,
) -> Result<Manager<SqliteStore, Registered>, SignalError> {
    Manager::link_secondary_device(store, SignalServers::Production, device_name.to_string(), provisioning_link)
        .await
}

/// Loads an already linked device from the store.
pub async fn load_registered(store: SqliteStore) -> Result<Manager<SqliteStore, Registered>, SignalError> {
    Manager::load_registered(store).await
}

/// Loads the linked device from the store, or links one if there is none yet.
///
/// When linking, `show_link` gets the provisioning URL to display (as a QR code) for the
/// phone to scan. Returns the manager and whether a new link was just made.
pub async fn link_or_load(
    store: SqliteStore,
    device_name: &str,
    show_link: impl FnOnce(&Url),
) -> Result<(SignalManager, bool), SignalError> {
    match load_registered(store.clone()).await {
        Ok(manager) => Ok((manager, false)),
        Err(presage::Error::NotYetRegisteredError) => {
            let (tx, rx) = oneshot::channel();
            let show = async {
                if let Ok(url) = rx.await {
                    show_link(&url);
                }
            };
            let (manager, ()) = future::join(link_device(store, device_name, tx), show).await;
            Ok((manager?, true))
        }
        Err(e) => Err(e),
    }
}

/// A conversation (1-1 or group) known to the store.
pub struct Conversation {
    pub thread: Thread,
    pub title: String,
    /// Position in the phone's conversation list (1 = top), from the contacts sync.
    /// Only known for 1-1 conversations.
    pub inbox_position: Option<u32>,
    /// Number of text messages stored for this conversation.
    pub message_count: usize,
    /// Time and text of the latest text message.
    pub last_message: Option<(DateTime<Local>, String)>,
}

/// Fetches what is waiting on the server (messages, contacts) and the groups listed in the
/// Storage Service into the store.
///
/// Also asks the phone for a contacts sync and waits up to `contacts_timeout` after the
/// queue is drained for it to arrive. Returns the conversations seen in received messages,
/// which may include people missing from the synchronized contacts.
pub async fn sync(
    manager: &mut Manager<SqliteStore, Registered>,
    contacts_timeout: Duration,
) -> Result<HashSet<Thread>, SignalError> {
    // Generous idle limit while the queue is drained: a fresh link can have a lot pending.
    const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

    manager.request_contacts().await?;
    let mut seen = HashSet::new();
    // Scoped so the receiving connection closes before the groups are fetched.
    {
        let mut messages = pin!(manager.receive_messages().await?);
        let mut contacts_deadline: Option<Instant> = None;
        let mut got_contacts = false;

        loop {
            let wait = match contacts_deadline {
                Some(_) if got_contacts => break,
                Some(deadline) => deadline.saturating_duration_since(Instant::now()),
                None => IDLE_TIMEOUT,
            };
            let Ok(Some(received)) = tokio::time::timeout(wait, messages.next()).await else { break };
            match received {
                Received::QueueEmpty => {
                    contacts_deadline.get_or_insert(Instant::now() + contacts_timeout);
                }
                Received::Contacts => got_contacts = true,
                Received::Content(content) => {
                    if let Ok(thread) = Thread::try_from(&*content) {
                        seen.insert(thread);
                    }
                }
                _ => {}
            }
        }
    }

    // Groups are not part of the contacts sync: the Storage Service lists them.
    manager.sync_groups_from_storage_service().await?;
    Ok(seen)
}

/// Lists the open conversations: stored groups, contacts that have a conversation on the
/// phone, and `extra` threads. Those with messages come first (latest first), then the
/// others in the phone's order.
///
/// Contacts without a conversation on the phone (inbox position 0 in the contacts sync)
/// are left out unless they have messages here or were seen during the last sync.
pub async fn conversations(
    manager: &Manager<SqliteStore, Registered>,
    extra: impl IntoIterator<Item = Thread>,
) -> Result<Vec<Conversation>, SqliteStoreError> {
    let store = manager.store();
    // (thread, inbox position, open even without messages here)
    let mut threads: Vec<(Thread, Option<u32>, bool)> = Vec::new();
    for (key, _) in store.groups().await?.flatten() {
        threads.push((Thread::Group(key), None, true));
    }
    for contact in store.contacts().await?.flatten() {
        let position = Some(contact.inbox_position).filter(|&p| p > 0);
        threads.push((Thread::Contact(ServiceId::Aci(contact.uuid.into())), position, position.is_some()));
    }
    for thread in extra {
        match threads.iter_mut().find(|(t, ..)| *t == thread) {
            Some((.., open)) => *open = true,
            None => threads.push((thread, None, true)),
        }
    }

    let mut conversations = Vec::new();
    for (thread, inbox_position, open) in threads {
        let title = match &thread {
            Thread::Group(key) => store.group(*key).await?.map(|g| g.title),
            Thread::Contact(id) => store.contact_by_id(id).await?.and_then(|c| {
                Some(c.name)
                    .filter(|n| !n.is_empty())
                    .or_else(|| c.phone_number.map(|p| p.to_string()))
            }),
        };
        let title = title.unwrap_or_else(|| match &thread {
            Thread::Group(_) => "<unknown group>".to_string(),
            Thread::Contact(id) => id.service_id_string(),
        });

        let mut message_count = 0;
        let mut last_message = None;
        for content in store.messages(&thread, ..).await?.flatten() {
            if let Some(text) = message_text(&content) {
                message_count += 1;
                let time = content.metadata.client_timestamp.with_timezone(&Local);
                if last_message.as_ref().is_none_or(|(t, _)| time >= *t) {
                    last_message = Some((time, text.to_string()));
                }
            }
        }
        if !open && message_count == 0 {
            continue;
        }
        conversations.push(Conversation { thread, title, inbox_position, message_count, last_message });
    }

    conversations.sort_by(|a, b| {
        let time = |c: &Conversation| c.last_message.as_ref().map(|(t, _)| *t);
        // `None` sorts first with `Option`'s order, hence the `u32::MAX` for unknown positions.
        let position = |c: &Conversation| c.inbox_position.unwrap_or(u32::MAX);
        time(b)
            .cmp(&time(a))
            .then_with(|| position(a).cmp(&position(b)))
            .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
    });
    Ok(conversations)
}

/// The data message of a content, whether received or sent by us from another device.
/// The flag tells whether it is a copy of a message we sent from another device.
fn data_message(content: &Content) -> Option<(&DataMessage, bool)> {
    match &content.body {
        ContentBody::DataMessage(message) => Some((message, false)),
        ContentBody::SynchronizeMessage(SyncMessage {
            content: Some(SyncContent::Sent(Sent { message: Some(message), .. })),
            ..
        }) => Some((message, true)),
        _ => None,
    }
}

/// Text of a message, whether received or sent by us from another device.
fn message_text(content: &Content) -> Option<&str> {
    data_message(content)?.0.body.as_deref()
}

/// Turns stored or received Signal messages into interface [`Message`]s.
pub struct Directory {
    own_aci: Uuid,
    /// Display names of the synchronized contacts.
    names: HashMap<Uuid, String>,
}

impl Directory {
    pub async fn load(manager: &SignalManager) -> Result<Self, SqliteStoreError> {
        let mut names = HashMap::new();
        for contact in manager.store().contacts().await?.flatten() {
            let name = Some(contact.name)
                .filter(|n| !n.is_empty())
                .or_else(|| contact.phone_number.map(|p| p.to_string()));
            if let Some(name) = name {
                names.insert(contact.uuid, name);
            }
        }
        Ok(Directory { own_aci: manager.registration_data().service_ids.aci, names })
    }

    /// The text message in `content`, with the id of the message it quotes.
    /// `None` for anything that is not a text message (receipts, typing, reactions…).
    pub fn to_message(&self, content: &Content) -> Option<(Message, Option<u64>)> {
        let (data, sent_elsewhere) = data_message(content)?;
        let text = data.body.clone()?;
        let sender = content.metadata.sender.raw_uuid();
        let from_me = sent_elsewhere || sender == self.own_aci;
        let id = data.timestamp.unwrap_or_else(|| content.metadata.client_timestamp.timestamp_millis() as u64);
        let sender_name = match self.names.get(&sender) {
            Some(name) => name.clone(),
            None => sender.to_string()[..8].to_string(),
        };
        let message = Message {
            id,
            author: Some(if from_me { self.own_aci } else { sender }.to_string()),
            from_me,
            sender_name,
            text,
            timestamp: content.metadata.client_timestamp.with_timezone(&Local).naive_local(),
            reply_to: None,
            forwarded: false,
        };
        Some((message, data.quote.as_ref().and_then(|q| q.id)))
    }

    /// Appends the message in `content` (if any) to `discussion`, resolving its quote.
    /// Returns whether a message was added.
    pub fn append(&self, discussion: &mut Discussion, content: &Content) -> bool {
        let Some(message) = self.resolve(discussion, content) else { return false };
        discussion.messages.push(message);
        true
    }

    /// The message in `content` (if any), with its quote resolved against `discussion`.
    pub fn resolve(&self, discussion: &Discussion, content: &Content) -> Option<Message> {
        let (mut message, quote) = self.to_message(content)?;
        message.reply_to = quote.and_then(|id| discussion.position_of(id));
        Some(message)
    }
}

/// Builds the interface discussions for `conversations`, with their stored messages.
/// The threads are returned in the same order, to know where to send.
pub async fn load_discussions(
    manager: &SignalManager,
    directory: &Directory,
    conversations: &[Conversation],
) -> Result<(Vec<Thread>, Vec<Discussion>), SqliteStoreError> {
    let mut threads = Vec::new();
    let mut discussions = Vec::new();
    for conversation in conversations {
        let mut discussion = Discussion { title: conversation.title.clone(), messages: Vec::new() };
        let mut contents: Vec<Content> = manager.store().messages(&conversation.thread, ..).await?.flatten().collect();
        contents.sort_by_key(|c| c.metadata.client_timestamp);
        for content in &contents {
            directory.append(&mut discussion, content);
        }
        threads.push(conversation.thread.clone());
        discussions.push(discussion);
    }
    Ok((threads, discussions))
}

/// Sends `text` to `thread`, quoting `quote` if given. Returns the sent message.
pub async fn send(
    manager: &mut SignalManager,
    thread: &Thread,
    text: &str,
    quote: Option<(usize, &Message)>,
) -> Result<Message, SignalError> {
    let timestamp = Utc::now().timestamp_millis() as u64;
    let mut message = DataMessage {
        body: Some(text.to_string()),
        timestamp: Some(timestamp),
        quote: quote.map(|(_, q)| Quote {
            id: Some(q.id),
            author_aci: q.author.clone(),
            text: Some(q.text.clone()),
            ..Default::default()
        }),
        ..Default::default()
    };
    match thread {
        Thread::Contact(recipient) => manager.send_message(*recipient, message, timestamp).await?,
        Thread::Group(master_key) => {
            let revision = manager.store().group(*master_key).await?.map_or(0, |g| g.revision);
            message.group_v2 = Some(GroupContextV2 {
                master_key: Some(master_key.to_vec()),
                revision: Some(revision),
                ..Default::default()
            });
            manager.send_message_to_group(master_key, message, timestamp).await?
        }
    }
    let mut sent = Message::mine(timestamp, text.to_string(), quote.map(|(i, _)| i), false);
    sent.author = Some(manager.registration_data().service_ids.aci.to_string());
    Ok(sent)
}
