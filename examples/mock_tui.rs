//! The interface with fake discussions, no Signal involved: sending only appends locally.
//!
//!   cargo run --example mock_tui

use std::io::{self, Write, stdout};

use crossterm::event::{self, Event};
use crossterm::terminal;
use mysignalcli::data::{self, Message};
use mysignalcli::tui::{Action, App, TerminalGuard};

fn main() -> io::Result<()> {
    let mut app = App::new(data::mock_discussions());
    let _terminal = TerminalGuard::enter()?;
    let mut out = stdout();
    let mut next_id = 1;

    loop {
        let (w, h) = terminal::size()?;
        app.draw(&mut out, w, h)?;
        out.flush()?;

        let Event::Key(key) = event::read()? else { continue };
        match app.handle_key(key) {
            Some(Action::Quit) => return Ok(()),
            Some(Action::Opened(_)) => {}
            Some(Action::Send { discussion, text, reply_to }) => {
                app.push_message(discussion, Message::mine(next_id, text, reply_to, false));
                next_id += 1;
            }
            Some(Action::Forward { from, msg, to }) => {
                let text = app.discussions[from].messages[msg].text.clone();
                app.push_message(to, Message::mine(next_id, text, None, true));
                next_id += 1;
                app.set_status(format!("Forwarded to {}", app.discussions[to].title));
            }
            None => {}
        }
    }
}
