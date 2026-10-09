//! Links this client to your Signal account if needed, then prints your conversations.
//!
//!   cargo run --example link_device [-- --reset]
//!
//! Same store as signal-tui (`~/.local/share/signal-tui/`, or `$SIGNAL_TUI_DB`).
//! `--reset` deletes it first, to link again from scratch (also remove the old device
//! from the phone).
//!
//! Signal does not hand the message history to linked devices: conversations come from
//! the contacts and groups synced by the phone, and only messages received since the
//! link are known.

use std::time::Duration;

use signal_tui::{qr, signal};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = signal::default_store_path().expect("cannot create the store directory");
    let db = path.to_str().expect("the store path is not valid UTF-8");

    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("--reset") => {
            signal::remove_store(db).expect("cannot delete the store");
            println!("Store {db} deleted. Also remove the old \"signal-tui\" device from the phone (Settings > Linked devices).");
        }
        Some(arg) => {
            eprintln!("unknown argument {arg:?}\nusage: link_device [--reset]");
            std::process::exit(2);
        }
    }

    let store = signal_tui::store_key::open_encrypted(db).await.unwrap();
    let (mut manager, _) = signal::link_or_load(store, "signal-tui", |url| {
        println!("Scan this QR code from Signal on your phone (Settings > Linked devices):");
        qr::print_qr(url.as_str());
        println!("Or use the URL: {url}");
    })
    .await
    .expect("device linking failed");

    let whoami = manager.whoami().await.expect("whoami request failed");
    eprintln!("Connected as {}. Syncing\u{2026}", whoami.aci);

    let seen = signal::sync(&mut manager, Duration::from_secs(15)).await.expect("sync failed");
    let conversations = signal::conversations(&manager, seen).await.expect("cannot read the store");

    // One line per conversation: date of the last message, title, message count, last message.
    for c in &conversations {
        let date = c.last_message.as_ref().map_or_else(|| " ".repeat(16), |(t, _)| t.format("%Y-%m-%d %H:%M").to_string());
        let kind = match c.thread {
            signal::Thread::Group(_) => "group",
            signal::Thread::Contact(_) => "",
        };
        let last = c.last_message.as_ref().map_or(String::new(), |(_, text)| text.replace('\n', " "));
        println!("{date}  {:<30} {kind:<5} {:>4} msg  {last}", c.title, c.message_count);
    }
    eprintln!("{} conversation(s)", conversations.len());
}
