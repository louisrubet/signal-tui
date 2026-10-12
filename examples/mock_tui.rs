//! The interface with fake discussions, no Signal involved: sending only appends locally.
//!
//!   cargo run --example mock_tui            # `-- --open` opens the first chat

use std::io::{self, Write, stdout};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal;
use signal_tui::data::{self, Message};
use signal_tui::graphics::Thumbnails;
use signal_tui::tui::{Action, App, TerminalGuard};

fn main() -> io::Result<()> {
    let mut discussions = data::mock_discussions();
    discussions[0].pinned = Some(1);
    discussions.insert(0, demo::discussion());
    let mut app = App::new(discussions);
    app.thumbnails = Thumbnails::detect();
    if std::env::args().any(|arg| arg == "--open") {
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }
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

/// A chat showing what the interface can do: photos, link previews, emojis, reactions
/// (the screenshot of the README).
mod demo {
    use chrono::{Duration, Local, NaiveTime};
    use image::{Rgb, RgbImage};
    use signal_tui::data::{Attachment, Discussion, LinkPreview, Message, Style, Styled};

    pub fn discussion() -> Discussion {
        let today = Local::now().date_naive();
        let at = |days: i64, h: u32, m: u32| {
            (today - Duration::days(days)).and_time(NaiveTime::from_hms_opt(h, m, 0).unwrap())
        };
        let from = |name: &str, text: &str, time| Message {
            from_me: false,
            author: Some(name.to_string()),
            sender_name: name.to_string(),
            timestamp: time,
            ..Message::mine(0, text.to_string(), None, false)
        };
        let mine = |text: &str, time| Message {
            timestamp: time,
            ..Message::mine(0, text.to_string(), None, false)
        };

        let mut sunset = from(
            "Alice Martin",
            "Landed! The light here is unreal \u{1f305}",
            at(0, 9, 42),
        );
        sunset.attachments = vec![jpeg(include_bytes!("demo/sunset.jpg"))];
        sunset.set_reaction("me", true, Some("\u{2764}\u{fe0f}"));
        sunset.set_reaction("Bob Dupont", false, Some("\u{1f60d}"));

        let mut alfama = from(
            "Bob Dupont",
            "Dinner tonight is here: https://en.wikipedia.org/wiki/Alfama",
            at(0, 17, 5),
        );
        alfama.previews = vec![LinkPreview {
            url: "https://en.wikipedia.org/wiki/Alfama".to_string(),
            title: "Alfama - Wikipedia".to_string(),
            description: "The oldest neighborhood of Lisbon, spreading on the slope between the \
                          S\u{e3}o Jorge Castle and the Tagus river."
                .to_string(),
            image: Some(jpeg(include_bytes!("demo/alfama.jpg"))),
            remote_image: false,
        }];

        let mut booked = from(
            "Alice Martin",
            "Perfect, I'll book for 8 \u{1f377}",
            at(0, 17, 9),
        );
        booked.reply_to = Some(2);
        booked.styles = vec![Styled {
            start: 0,
            end: 7,
            style: Style::Bold,
        }];

        let mut app = mine(
            "The app I told you about \u{1f447} https://github.com/louisrubet/signal-tui",
            at(0, 17, 30),
        );
        app.previews = vec![LinkPreview {
            url: "https://github.com/louisrubet/signal-tui".to_string(),
            title: "louisrubet/signal-tui".to_string(),
            description: "A Signal client for the terminal: thumbnails, link previews, \
                          reactions, replies."
                .to_string(),
            image: Some(png(card_image(480, 240))),
            remote_image: false,
        }];
        app.set_reaction("Alice Martin", false, Some("\u{1f44d}"));

        let mut tram = from("Bob Dupont", "Tram 28, obviously \u{1f68b}", at(0, 18, 2));
        tram.styles = vec![Styled {
            start: 9,
            end: 18,
            style: Style::Italic,
        }];
        tram.attachments = vec![jpeg(include_bytes!("demo/tram.jpg"))];

        let mut messages = vec![
            sunset,
            mine("Wow \u{1f60d} save me a pastel de nata!", at(0, 9, 45)),
            alfama,
            booked,
            app,
            tram,
        ];
        // Ids are send times, as in Signal.
        for message in &mut messages {
            message.id = message.timestamp.and_utc().timestamp_millis() as u64;
        }
        Discussion {
            title: "Lisbon trip \u{1f30d}".to_string(),
            messages,
            unread: 1,
            pinned: Some(0),
        }
    }

    /// A photo of `examples/demo` (credits in its README).
    fn jpeg(bytes: &[u8]) -> Attachment {
        Attachment {
            content_type: "image/jpeg".to_string(),
            file_name: None,
            width: None,
            height: None,
            bytes: bytes.to_vec(),
        }
    }

    fn png(image: RgbImage) -> Attachment {
        let mut bytes = Vec::new();
        image
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("PNG encoding");
        Attachment {
            content_type: "image/png".to_string(),
            file_name: None,
            width: Some(image.width()),
            height: Some(image.height()),
            bytes,
        }
    }

    fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> Rgb<u8> {
        let t = t.clamp(0.0, 1.0);
        Rgb([0, 1, 2].map(|i| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t) as u8))
    }

    /// A social card: a terminal window on a dark background.
    fn card_image(w: u32, h: u32) -> RgbImage {
        let (left, top, right, bottom) = (w / 6, h / 5, w * 5 / 6, h * 4 / 5);
        RgbImage::from_fn(w, h, |x, y| {
            let inside = (left..right).contains(&x) && (top..bottom).contains(&y);
            if !inside {
                return mix([13, 17, 23], [40, 50, 90], (x + y) as f32 / (w + h) as f32);
            }
            if y < top + 18 {
                let dot = |i: u32| {
                    (x as i32 - (left + 16 + i * 18) as i32).pow(2)
                        + (y as i32 - (top + 9) as i32).pow(2)
                        < 30
                };
                if dot(0) {
                    return Rgb([255, 95, 86]);
                }
                if dot(1) {
                    return Rgb([255, 189, 46]);
                }
                if dot(2) {
                    return Rgb([39, 201, 63]);
                }
                return Rgb([60, 64, 72]);
            }
            let line = (y - top - 18) / 22;
            let col = x - left;
            let lengths = [140, 220, 90, 180, 120];
            if (y - top - 18) % 22 > 8
                && (y - top - 18) % 22 < 16
                && col > 16
                && col < 16 + lengths[line as usize % 5]
            {
                return if line == 0 {
                    Rgb([58, 118, 240])
                } else {
                    Rgb([170, 180, 190])
                };
            }
            Rgb([30, 33, 40])
        })
    }
}
