//! signal-tui: a terminal Signal client, linked as a secondary device of your phone.
//!
//!   signal-tui [--reset]
//!
//! On the first run, scan the QR code from the phone (Settings > Linked devices). The store
//! lives in `~/.local/share/signal-tui/` (or `$SIGNAL_TUI_DB`). `--reset` deletes it first,
//! to link again from scratch.

use std::error::Error;
use std::io::{Write, stdout};
use std::pin::pin;
use std::time::Duration;

use crossterm::event::{Event, EventStream};
use crossterm::terminal;
use futures::StreamExt;
use mysignalcli::data::Discussion;
use mysignalcli::signal::{self, Directory, Thread};
use mysignalcli::tui::{Action, App, TerminalGuard};
use mysignalcli::qr;
use presage::model::messages::Received;

const USAGE: &str = "usage: signal-tui [--reset]";
const DEVICE_NAME: &str = "signal-tui";

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(e) = run().await {
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
        eprintln!("Store {db} deleted. Also remove the old \"{DEVICE_NAME}\" device from the phone (Settings > Linked devices).");
    }

    let store = signal::open_store(db, None).await?;
    let (mut manager, linked) = signal::link_or_load(store, DEVICE_NAME, |url| {
        println!("Scan this QR code from Signal on your phone (Settings > Linked devices):");
        qr::print_qr(url.as_str());
        println!("Or use the URL: {url}");
    })
    .await?;

    eprintln!("Syncing\u{2026}");
    // Right after linking, give the phone time to send the contacts.
    let contacts_timeout = Duration::from_secs(if linked { 15 } else { 2 });
    let seen = signal::sync(&mut manager, contacts_timeout).await?;
    let conversations = signal::conversations(&manager, seen).await?;
    let directory = Directory::load(&manager).await?;
    let (mut threads, discussions) = signal::load_discussions(&manager, &directory, &conversations).await?;
    let mut app = App::new(discussions);

    // Messages arriving while the interface runs, on a clone so `manager` stays free to send.
    let mut receiver = manager.clone();
    let mut incoming = pin!(receiver.receive_messages().await?);
    let mut incoming_open = true;
    let mut keys = EventStream::new();

    let _terminal = TerminalGuard::enter()?;
    let mut out = stdout();
    loop {
        let (w, h) = terminal::size()?;
        app.draw(&mut out, w, h)?;
        out.flush()?;

        tokio::select! {
            event = keys.next() => {
                let Some(event) = event else { break };
                let Event::Key(key) = event? else { continue };
                let Some(action) = app.handle_key(key) else { continue };

                let (discussion, text, reply_to, forwarded) = match action {
                    Action::Quit => break,
                    Action::Send { discussion, text, reply_to } => (discussion, text, reply_to, false),
                    Action::Forward { from, msg, to } => (to, app.discussions[from].messages[msg].text.clone(), None, true),
                };
                app.set_status("Sending\u{2026}");
                app.draw(&mut out, w, h)?;
                out.flush()?;

                let quote = reply_to.map(|i| (i, &app.discussions[discussion].messages[i]));
                match signal::send(&mut manager, &threads[discussion], &text, quote).await {
                    Ok(mut message) => {
                        message.forwarded = forwarded;
                        app.push_message(discussion, message);
                        if forwarded {
                            app.set_status(format!("Forwarded to {}", app.discussions[discussion].title));
                        } else {
                            app.set_status("Sent");
                        }
                    }
                    Err(e) => app.set_status(format!("Sending failed: {e}")),
                }
            }
            received = incoming.next(), if incoming_open => {
                let Some(received) = received else {
                    incoming_open = false;
                    app.set_status("Disconnected from Signal: restart to receive new messages");
                    continue;
                };
                let Received::Content(content) = received else { continue };
                let Ok(thread) = Thread::try_from(&*content) else { continue };
                if directory.to_message(&content).is_none() {
                    continue;
                }
                let idx = match threads.iter().position(|t| *t == thread) {
                    Some(idx) => idx,
                    None => {
                        // A conversation we did not list yet (e.g. someone not in the contacts).
                        let title = manager.thread_title(&thread).await.unwrap_or_default();
                        let title = if title.is_empty() { thread.to_string() } else { title };
                        threads.push(thread);
                        app.push_discussion(Discussion { title, messages: Vec::new() })
                    }
                };
                if let Some(message) = directory.resolve(&app.discussions[idx], &content) {
                    app.push_message(idx, message);
                }
            }
        }
    }
    Ok(())
}
