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
use std::time::{Duration, Instant};

use crossterm::event::{Event, EventStream};
use crossterm::terminal;
use futures::StreamExt;
use signal_tui::data::{Discussion, Message};
use signal_tui::settings::Settings;
use signal_tui::signal::{self, Directory, Pins, ReadMarks, Thread};
use signal_tui::tui::{Action, App, TerminalGuard};
use signal_tui::qr;
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
    let (mut threads, mut discussions) = signal::load_discussions(&manager, &directory, &conversations).await?;
    let mut read_marks = ReadMarks::load(path.with_extension("read"));
    let mut pins = Pins::load(path.with_extension("pinned"));
    for (thread, discussion) in threads.iter().zip(&mut discussions) {
        discussion.unread = read_marks.is_unread(thread, discussion);
        discussion.pinned = pins.rank(thread);
    }
    let mut app = App::new(discussions);
    let settings_path = Settings::default_path()?;
    app.settings = Settings::load(&settings_path);

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

        let typing_expiry = app.next_typing_expiry();
        tokio::select! {
            _ = tokio::time::sleep_until(typing_expiry.unwrap_or_else(Instant::now).into()), if typing_expiry.is_some() => {
                app.expire_typing(Instant::now());
            }
            event = keys.next() => {
                let Some(event) = event else { break };
                let action = match event? {
                    Event::Key(key) => app.handle_key(key),
                    Event::Paste(text) => {
                        app.handle_paste(&text);
                        None
                    }
                    _ => None,
                };
                let Some(action) = action else { continue };

                let (discussion, text, reply_to, forwarded) = match action {
                    Action::Quit => break,
                    Action::React { discussion, msg, emoji, remove } => {
                        let target = &app.discussions[discussion].messages[msg];
                        match signal::react(&mut manager, &threads[discussion], target, &emoji, remove).await {
                            Ok(update) => {
                                update.apply(&mut app.discussions[discussion]);
                            }
                            Err(e) => app.set_status(format!("Reaction failed: {e}")),
                        }
                        continue;
                    }
                    Action::PinToggled(idx) => {
                        if let Err(e) = pins.set(&threads[idx], app.discussions[idx].pinned.is_some()) {
                            app.set_status(format!("Cannot save the pinned chats: {e}"));
                        }
                        continue;
                    }
                    Action::SettingsChanged => {
                        if let Err(e) = app.settings.save(&settings_path) {
                            app.set_status(format!("Cannot save the parameters: {e}"));
                        }
                        continue;
                    }
                    Action::Opened(idx) => {
                        if let Err(e) = read_marks.mark_read(&threads[idx], &app.discussions[idx]) {
                            app.set_status(format!("Cannot save the read state: {e}"));
                        }
                        continue;
                    }
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
                if let Some(typing) = directory.typing(&content) {
                    if let Some(idx) = threads.iter().position(|t| *t == typing.thread) {
                        app.set_typing(idx, &typing.author, &typing.name, typing.started, Instant::now());
                    }
                    continue;
                }
                let Ok(thread) = Thread::try_from(&*content) else { continue };
                if let Some(reaction) = directory.reaction(&content) {
                    if let Some(idx) = threads.iter().position(|t| *t == thread) {
                        reaction.apply(&mut app.discussions[idx]);
                    }
                    continue;
                }
                let is_group = matches!(thread, Thread::Group(_));
                if directory.to_message(&content).is_none() {
                    continue;
                }
                let idx = match threads.iter().position(|t| *t == thread) {
                    Some(idx) => idx,
                    None => {
                        // A conversation we did not list yet (e.g. someone not in the contacts).
                        let title = signal::thread_title(&manager, &thread).await.unwrap_or_else(|_| thread.to_string());
                        threads.push(thread);
                        app.push_discussion(Discussion::new(title))
                    }
                };
                if let Some(message) = directory.resolve(&app.discussions[idx], &content) {
                    if app.settings.notifications && !message.from_me && !app.is_open(idx) {
                        notify(&app.discussions[idx], &message, is_group);
                    }
                    app.push_message(idx, message);
                    // Arrived in the discussion being read: no star on the next start.
                    if app.is_open(idx) && let Err(e) = read_marks.mark_read(&threads[idx], &app.discussions[idx]) {
                        app.set_status(format!("Cannot save the read state: {e}"));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Desktop notification for a received message: the conversation as title, the text as body
/// (prefixed by the sender in groups).
fn notify(discussion: &Discussion, message: &Message, is_group: bool) {
    let summary = discussion.title.clone();
    let body = if is_group { format!("{}: {}", message.sender_name, message.text) } else { message.text.clone() };
    // Notification servers block on D-Bus / OS calls: keep them off the event loop.
    // Failures (no notification server…) are not worth interrupting the user for.
    tokio::task::spawn_blocking(move || {
        let _ = notify_rust::Notification::new()
            .appname("signal-tui")
            .summary(&summary)
            .body(&escape_markup(&body))
            .show();
    });
}

/// The freedesktop notification body is markup: `<`, `>` and `&` must be escaped there.
fn escape_markup(text: &str) -> String {
    if cfg!(all(unix, not(target_os = "macos"))) {
        text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
    } else {
        text.to_string()
    }
}
