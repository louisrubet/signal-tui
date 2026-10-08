//! Links this client to your Signal account if needed, then prints your conversations.
//!
//!   cargo run --example link_device [-- --reset]
//!
//! On the first run, scan the QR code from the phone (Settings > Linked devices).
//! The store is kept in `$SIGNAL_TUI_DB` (default `signal-tui.db3`), so later runs
//! reuse the link instead of asking again. `--reset` deletes the store first, to link
//! again from scratch (also remove the old device from the phone).
//!
//! Signal does not hand the message history to linked devices: conversations come from
//! the contacts and groups synced by the phone, and only messages received since the
//! link are known.

use std::time::Duration;

use crossterm::style::{Color, Stylize};
use futures::channel::oneshot;
use futures::future;
use mysignalcli::signal;
use qrcode::QrCode;

/// Prints `data` as a QR code in true black on true white.
///
/// The named ANSI colors (what qr2term uses) follow the terminal theme and can come out
/// e.g. purple, which the Signal app struggles to scan; explicit RGB colors do not.
fn print_qr(data: &str) {
    const QUIET_ZONE: usize = 4;
    const BLACK: Color = Color::Rgb { r: 0, g: 0, b: 0 };
    const WHITE: Color = Color::Rgb { r: 255, g: 255, b: 255 };

    let code = QrCode::new(data).expect("data too long for a QR code");
    let width = code.width();
    let colors = code.to_colors();
    let size = width + 2 * QUIET_ZONE;
    let is_dark = |x: usize, y: usize| {
        let (x, y) = (x.wrapping_sub(QUIET_ZONE), y.wrapping_sub(QUIET_ZONE));
        x < width && y < width && colors[y * width + x] == qrcode::Color::Dark
    };
    let color = |dark: bool| if dark { BLACK } else { WHITE };

    // Each character holds two modules: the upper one as foreground of '▀', the lower as background.
    for y in (0..size).step_by(2) {
        let line: String = (0..size)
            .map(|x| format!("{}", '▀'.with(color(is_dark(x, y))).on(color(is_dark(x, y + 1)))))
            .collect();
        println!("{line}");
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let db = std::env::var("SIGNAL_TUI_DB").unwrap_or_else(|_| "signal-tui.db3".to_string());

    match std::env::args().nth(1).as_deref() {
        None => {}
        Some("--reset") => {
            signal::remove_store(&db).expect("cannot delete the store");
            println!("Store {db} deleted. Also remove the old \"signal-tui\" device from the phone (Settings > Linked devices).");
        }
        Some(arg) => {
            eprintln!("unknown argument {arg:?}\nusage: link_device [--reset]");
            std::process::exit(2);
        }
    }

    let store = signal::open_store(&db, None).await.unwrap();

    let mut manager = match signal::load_registered(store.clone()).await {
        Ok(manager) => manager,
        Err(presage::Error::NotYetRegisteredError) => {
            let (tx, rx) = oneshot::channel::<url::Url>();
            let show_qr = async {
                if let Ok(url) = rx.await {
                    println!("Scan this QR code from Signal on your phone (Settings > Linked devices):");
                    print_qr(url.as_str());
                    println!("Or use the URL: {url}");
                }
            };
            let (manager, ()) = future::join(signal::link_device(store, "signal-tui", tx), show_qr).await;
            manager.expect("device linking failed")
        }
        Err(e) => panic!("cannot load the store {db}: {e:?}"),
    };

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
