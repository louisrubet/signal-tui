//! The interface with fake discussions, no Signal involved: sending only appends locally.
//!
//!   cargo run --example mock_tui

use std::io::{self, Write, stdout};

use crossterm::event::{self, Event};
use crossterm::terminal;
use signal_tui::data::{self, Message};
use signal_tui::graphics::Thumbnails;
use signal_tui::tui::{Action, App, TerminalGuard};

fn main() -> io::Result<()> {
    let mut app = App::new(data::mock_discussions());
    app.thumbnails = Thumbnails::detect();
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
            Some(Action::Quit) => return app.thumbnails.clear(&mut out),
            Some(Action::Delete { discussion, msg }) => {
                app.discussions[discussion].messages[msg].mark_deleted()
            }
            Some(Action::React {
                discussion,
                msg,
                emoji,
                remove,
            }) => {
                let emoji = (!remove).then_some(emoji.as_str());
                app.discussions[discussion].messages[msg].set_reaction("me", true, emoji);
            }
            Some(
                Action::Opened(_)
                | Action::SettingsChanged
                | Action::PinToggled(_)
                | Action::FetchAttachments { .. },
            ) => {}
            Some(Action::PasteImageFromClipboard) => {
                app.handle_pasted_image(data::Attachment::from_clipboard().unwrap_or_else(gradient))
            }
            Some(Action::Send {
                discussion,
                text,
                styles,
                reply_to,
            }) => {
                let mut message = Message::mine(next_id, text, reply_to, false);
                message.styles = styles;
                app.push_message(discussion, message);
                next_id += 1;
            }
            Some(Action::SendWithAttachments {
                discussion,
                text,
                attachments,
                reply_to,
            }) => {
                let attachment_count = attachments.len();
                let mut message = Message::mine(next_id, text, reply_to, false);
                message.attachments = attachments;
                if attachment_count > 0 {
                    app.set_status(format!("Mock sent {attachment_count} attachment(s)"));
                }
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

/// A color gradient, pasted when the clipboard holds no image.
fn gradient() -> data::Attachment {
    let (width, height) = (320, 200);
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 255 / width) as u8, (y * 255 / height) as u8, 160])
    });
    let mut bytes = Vec::new();
    image
        .write_to(&mut io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .expect("PNG encoding");
    data::Attachment {
        content_type: "image/png".to_string(),
        file_name: None,
        bytes,
        width: Some(width),
        height: Some(height),
    }
}
