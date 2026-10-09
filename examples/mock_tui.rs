//! The interface with fake discussions, no Signal involved: sending only appends locally.
//!
//!   cargo run --example mock_tui

use std::io::{self, Write, stdout};

use crossterm::event::{self, Event};
use crossterm::terminal;
use signal_tui::data::{self, Message};
use signal_tui::tui::{Action, App, TerminalGuard};

fn main() -> io::Result<()> {
    let mut app = App::new(data::mock_discussions());
    let _terminal = TerminalGuard::enter()?;
    let mut out = stdout();
    // Message ids are send times in ms, as in Signal (deletion is allowed for 24 h).
    let mut next_id = chrono::Utc::now().timestamp_millis() as u64;

    loop {
        let (w, h) = terminal::size()?;
        app.draw(&mut out, w, h)?;
        out.flush()?;

        let action = match event::read()? {
            Event::Key(key) => app.handle_key(key),
            Event::Paste(text) => {
                app.handle_paste(&text);
                None
            }
            _ => None,
        };
        match action {
            Some(Action::Quit) => return Ok(()),
            Some(Action::Delete { discussion, msg }) => app.discussions[discussion].messages[msg].mark_deleted(),
            Some(Action::React { discussion, msg, emoji, remove }) => {
                let emoji = (!remove).then_some(emoji.as_str());
                app.discussions[discussion].messages[msg].set_reaction("me", true, emoji);
            }
            Some(Action::Opened(_) | Action::SettingsChanged | Action::PinToggled(_)) => {}
            Some(Action::Send { discussion, text, styles, reply_to }) => {
                let mut message = Message::mine(next_id, text, reply_to, false);
                message.styles = styles;
                app.push_message(discussion, message);
                next_id += 1;
            }
            Some(Action::Forward { from, msg, to }) => {
                let original = &app.discussions[from].messages[msg];
                let mut message = Message::mine(next_id, original.text.clone(), None, true);
                message.styles = original.styles.clone();
                app.push_message(to, message);
                next_id += 1;
                app.set_status(format!("Forwarded to {}", app.discussions[to].title));
            }
            None => {}
        }
    }
}
