//! Connection to the Signal servers through presage, as a linked (secondary) device.

use std::collections::HashSet;
use std::pin::pin;
use std::time::Duration;

use chrono::{DateTime, Local};
use futures::StreamExt;
use futures::channel::oneshot;
use presage::Manager;
use presage::libsignal_service::configuration::SignalServers;
use presage::libsignal_service::content::{Content, ContentBody};
use presage::libsignal_service::protocol::ServiceId;
use presage::manager::Registered;
use presage::model::identity::OnNewIdentity;
use presage::model::messages::Received;
use presage::proto::SyncMessage;
use presage::proto::sync_message::{Content as SyncContent, Sent};
use presage::store::ContentsStore;
pub use presage::store::Thread;
use tokio::time::Instant;
use presage_store_sqlite::{SqliteStore, SqliteStoreError};
use url::Url;

pub type SignalError = presage::Error<SqliteStoreError>;

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
    let mut messages = pin!(manager.receive_messages().await?);
    let mut seen = HashSet::new();
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
    drop(messages);

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

/// Text of a message, whether received or sent by us from another device.
fn message_text(content: &Content) -> Option<&str> {
    let message = match &content.body {
        ContentBody::DataMessage(message) => message,
        ContentBody::SynchronizeMessage(SyncMessage {
            content: Some(SyncContent::Sent(Sent { message: Some(message), .. })),
            ..
        }) => message,
        _ => return None,
    };
    message.body.as_deref()
}
