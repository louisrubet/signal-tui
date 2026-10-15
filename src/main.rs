//! signal-tui: a terminal Signal client, linked as a secondary device of your phone.
//!
//!   signal-tui [--reset | --version]
//!
//! On the first run, scan the QR code from the phone (Settings > Linked devices). The store
//! lives in `~/.local/share/signal-tui/` (or `$SIGNAL_TUI_DB`). `--reset` deletes it first,
//! to link again from scratch.

use std::collections::HashSet;
use std::error::Error;
use std::future::Future;
use std::path::Path;
use std::pin::pin;
use std::time::Duration;

use chat_tui::backend::Result as BackendResult;
use chat_tui::data::{ChatId, Discussion, Message};
use chat_tui::{Backend, Capabilities, Downloaded, Draft, Event, qr};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use presage::model::messages::Received;
use presage_store_sqlite::SqliteStore;
use signal_tui::signal::{self, Directory, SignalManager, Thread};

const USAGE: &str = "usage: signal-tui [--reset | --version]";
const DEVICE_NAME: &str = "signal-tui";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() == 2 && args[1] == "--version" {
        println!("{}", env!("SIGNAL_TUI_VERSION"));
        return;
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("cannot create the Tokio runtime");
    let local = tokio::task::LocalSet::new();

    if let Err(e) = runtime.block_on(local.run_until(run())) {
        eprintln!("signal-tui: {e}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let reset = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--reset") => true,
        Some(arg) => return Err(format!("unknown argument {arg:?}\n{USAGE}").into()),
    };

    let path = signal::default_store_path()?;
    let db = path.to_str().ok_or("the store path is not valid UTF-8")?;
    if reset {
        signal::remove_store(db)?;
        eprintln!(
            "Store {db} deleted. Also remove the old \"{DEVICE_NAME}\" device from the phone (Settings > Linked devices)."
        );
    }

    let store = signal_tui::store_key::open_encrypted(db).await?;
    let result = chat(store.clone(), &path).await;
    // Before exiting: SQLCipher crashes when a connection closes after its cleanup.
    let _ = tokio::time::timeout(Duration::from_secs(2), store.close()).await;
    result
}

/// Links or loads the account from `store` (at `path`), syncs, then runs the interface.
async fn chat(store: SqliteStore, path: &Path) -> Result<(), Box<dyn Error>> {
    let (manager, linked) = signal::link_or_load(store, DEVICE_NAME, |url| {
        println!("Scan this QR code from Signal on your phone (Settings > Linked devices):");
        qr::print_qr(url.as_str());
        println!("Or use the URL: {url}");
    })
    .await?;
    let mut manager = manager;

    eprintln!("Syncing\u{2026}");
    // Right after linking, give the phone time to send the contacts.
    let contacts_timeout = Duration::from_secs(if linked { 15 } else { 2 });
    let seen = signal::sync(&mut manager, contacts_timeout).await?;
    let conversations = signal::conversations(&manager, seen).await?;
    let directory = Directory::load(&manager).await?;
    let discussions = signal::load_discussions(&manager, &directory, &conversations).await?;

    let known = discussions.iter().map(|d| d.id.clone()).collect();
    let events = receive(manager.clone(), directory, known);
    // The read marks and pins stay next to the store, where earlier versions kept them.
    let mut config = chat_tui::Config::new(DEVICE_NAME)?;
    config.read_marks = path.with_extension("read");
    config.pins = path.with_extension("pinned");
    chat_tui::run(Signal { manager }, discussions, events, config).await?;
    Ok(())
}

/// Signal, for the interface.
struct Signal {
    manager: SignalManager,
}

/// The futures own a clone of the manager: they are `'static`, as the trait wants.
#[allow(clippy::manual_async_fn)]
impl Backend for Signal {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            delete_max_age: Some(Duration::from_millis(signal::DELETE_MAX_AGE_MS)),
        }
    }

    fn send(
        &self,
        chat: &ChatId,
        draft: Draft,
    ) -> impl Future<Output = BackendResult<Message>> + 'static {
        let (mut manager, thread) = (self.manager.clone(), signal::thread_of(chat));
        async move {
            let thread = thread.ok_or("unknown chat")?;
            let quote = draft.reply_to.as_ref();
            Ok(signal::send(
                &mut manager,
                &thread,
                &draft.text,
                &draft.styles,
                draft.attachments,
                quote,
            )
            .await?)
        }
    }

    fn react(
        &self,
        chat: &ChatId,
        message: &Message,
        emoji: &str,
        remove: bool,
    ) -> impl Future<Output = BackendResult<()>> + 'static {
        let (mut manager, thread) = (self.manager.clone(), signal::thread_of(chat));
        let (message, emoji) = (message.clone(), emoji.to_string());
        async move {
            let thread = thread.ok_or("unknown chat")?;
            signal::react(&mut manager, &thread, &message, &emoji, remove).await?;
            Ok(())
        }
    }

    fn delete(
        &self,
        chat: &ChatId,
        message: &Message,
    ) -> impl Future<Output = BackendResult<()>> + 'static {
        let (mut manager, thread) = (self.manager.clone(), signal::thread_of(chat));
        let message = message.clone();
        async move {
            let thread = thread.ok_or("unknown chat")?;
            Ok(signal::delete(&mut manager, &thread, &message).await?)
        }
    }

    fn download(
        &self,
        chat: &ChatId,
        message: &Message,
    ) -> impl Future<Output = BackendResult<Downloaded>> + 'static {
        let (manager, thread) = (self.manager.clone(), signal::thread_of(chat));
        let id = signal::signal_id(message);
        async move {
            let thread = thread.ok_or("unknown chat")?;
            Ok(signal::download_attachments(&manager, &thread, id).await?)
        }
    }
}

/// What arrives from Signal while the interface runs, as interface events. `known`: the
/// chats already listed. Ends when the connection is lost.
fn receive(
    mut manager: SignalManager,
    directory: Directory,
    mut known: HashSet<ChatId>,
) -> UnboundedReceiver<Event> {
    let (events, received) = unbounded();
    // Receiving borrows `manager`: downloads and titles go through clones.
    let other = manager.clone();
    tokio::task::spawn_local(async move {
        let incoming = match manager.receive_messages().await {
            Ok(incoming) => incoming,
            Err(e) => {
                let _ =
                    events.unbounded_send(Event::Status(format!("Cannot receive messages: {e}")));
                return;
            }
        };
        let mut incoming = pin!(incoming);
        while let Some(received) = incoming.next().await {
            let Received::Content(content) = received else {
                continue;
            };
            let event = if let Some(typing) = directory.typing(&content) {
                Event::Typing {
                    chat: signal::thread_key(&typing.thread),
                    author: typing.author,
                    name: typing.name,
                    started: typing.started,
                }
            } else {
                let Ok(thread) = Thread::try_from(&*content) else {
                    continue;
                };
                let chat = signal::thread_key(&thread);
                if let Some((target, author)) = directory.deletion(&content) {
                    Event::Deleted {
                        chat,
                        target: target.to_string(),
                        author,
                    }
                } else if let Some(reaction) = directory.reaction(&content) {
                    Event::Reaction {
                        chat,
                        target: reaction.target_id.to_string(),
                        author: reaction.author,
                        mine: reaction.mine,
                        emoji: reaction.emoji,
                    }
                } else if let Some((mut message, quote)) = directory.to_message(&content) {
                    if known.insert(chat.clone()) {
                        // A conversation not listed yet (e.g. someone not in the contacts).
                        let title = signal::thread_title(&other, &thread)
                            .await
                            .unwrap_or_else(|_| thread.to_string());
                        let mut discussion = Discussion::new(chat.clone(), title);
                        discussion.is_group = matches!(thread, Thread::Group(_));
                        let _ = events.unbounded_send(Event::Chat(discussion));
                    }
                    // Images only: videos and files can be big, they wait for `o`.
                    if message.needs_download() && message.images_only() {
                        match signal::download_content_attachments(&other, &content).await {
                            Ok((attachments, images)) => {
                                message.set_downloaded(attachments, images)
                            }
                            Err(_) => message.download_failed = true,
                        }
                    }
                    Event::Message {
                        chat,
                        message,
                        quote: quote.map(|id| id.to_string()),
                    }
                } else {
                    continue;
                }
            };
            if events.unbounded_send(event).is_err() {
                break;
            }
        }
    });
    received
}
