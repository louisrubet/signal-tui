//! Image thumbnails in the terminal, through the kitty graphics protocol (kitty, Ghostty).
//!
//! Each thumbnail is sent once as a virtual placement; its rows are then printed as
//! Unicode placeholders, ordinary text that scrolls and redraws like the messages.
//! See <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>.

use std::collections::HashMap;
use std::io::{self, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::ImageFormat;

use crate::data::Message;

/// Width of a thumbnail in pixels, as in Signal Desktop: from 200 to 300, the height
/// following up to 450.
const WIDTH: (u32, u32) = (200, 300);
const MAX_HEIGHT: u32 = 450;
/// Side of the icon of a link preview whose image is smaller than [`WIDTH`]`.0` both ways.
const ICON: u32 = 72;
/// Thumbnails of link previews come after the attachments, from this index.
const PREVIEWS: usize = 1 << 16;
/// Base64 bytes per escape sequence, as the protocol asks.
const CHUNK: usize = 4096;
/// Placeholder character: the cells of an image are drawn with it.
const PLACEHOLDER: char = '\u{10EEEE}';
/// Combining marks giving the row and column of a placeholder cell (start of kitty's
/// `rowcolumn-diacritics.txt`: thumbnails are at most 64 cells both ways).
const DIACRITICS: [char; 64] = [
    '\u{0305}', '\u{030D}', '\u{030E}', '\u{0310}', '\u{0312}', '\u{033D}', '\u{033E}', '\u{033F}',
    '\u{0346}', '\u{034A}', '\u{034B}', '\u{034C}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035B}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036A}', '\u{036B}', '\u{036C}', '\u{036D}', '\u{036E}', '\u{036F}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059C}', '\u{059D}', '\u{059E}', '\u{059F}', '\u{05A0}', '\u{05A1}',
    '\u{05A8}', '\u{05A9}', '\u{05AB}', '\u{05AC}', '\u{05AF}', '\u{05C4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
];

/// A thumbnail sent to the terminal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thumb {
    /// Image id in the terminal, also the color of its placeholders (24 bits).
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
}

impl Thumb {
    /// Row `row` of the thumbnail, at most `width` cells, as placeholder text.
    pub fn row(&self, row: u16, width: usize) -> String {
        let [_, r, g, b] = self.id.to_be_bytes();
        let mut text = format!("\x1b[38;2;{r};{g};{b}m");
        for col in DIACRITICS.iter().take((self.cols as usize).min(width)) {
            text.push(PLACEHOLDER);
            text.push(DIACRITICS[row as usize]);
            text.push(*col);
        }
        text.push_str("\x1b[39m");
        text
    }
}

/// The thumbnails of the images of the messages, sent as they are first shown.
#[derive(Default)]
pub struct Thumbnails {
    /// Whether the terminal speaks the kitty graphics protocol.
    enabled: bool,
    /// Size of a cell in pixels.
    cell: (u32, u32),
    next_id: u32,
    /// By (message id, attachment index); `None` when it cannot be shown.
    thumbs: HashMap<(u64, usize), Option<Thumb>>,
}

impl Thumbnails {
    /// Enabled in terminals known to support placeholders: kitty and Ghostty.
    pub fn detect() -> Self {
        let env = |name| std::env::var(name).unwrap_or_default();
        let enabled = env("TERM_PROGRAM") == "ghostty"
            || env("TERM") == "xterm-kitty"
            || std::env::var_os("KITTY_WINDOW_ID").is_some();
        let cell = crossterm::terminal::window_size()
            .ok()
            .filter(|s| s.width > 0 && s.height > 0 && s.columns > 0 && s.rows > 0)
            .map_or((10, 20), |s| {
                ((s.width / s.columns) as u32, (s.height / s.rows) as u32)
            });
        Thumbnails {
            enabled,
            cell,
            // Away from 0 and from ids other programs are likely to pick.
            next_id: 0x0051_0000 + rand::random::<u32>() % 0x1000,
            thumbs: HashMap::new(),
        }
    }

    /// Enabled, for tests.
    #[cfg(test)]
    pub fn on() -> Self {
        Thumbnails {
            enabled: true,
            cell: (10, 20),
            ..Default::default()
        }
    }

    /// Whether thumbnails are shown.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The thumbnail of attachment `index` of message `message_id`: `None` if not made
    /// yet, `Some(None)` if the image cannot be shown.
    pub fn get(&self, message_id: u64, index: usize) -> Option<Option<Thumb>> {
        self.thumbs.get(&(message_id, index)).copied()
    }

    /// The thumbnail of the image of link preview `index` of message `message_id`.
    pub fn preview(&self, message_id: u64, index: usize) -> Option<Thumb> {
        self.get(message_id, PREVIEWS + index).flatten()
    }

    /// Sends the thumbnails of the images of `messages` (attachments and link previews)
    /// not sent yet.
    pub fn send(&mut self, out: &mut impl Write, messages: &[Message]) -> io::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        for message in messages {
            let attachments = message
                .attachments
                .iter()
                .enumerate()
                .map(|(i, a)| (i, a, false));
            let previews = message
                .previews
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.image.as_ref().map(|image| (PREVIEWS + i, image, true)));
            for (index, attachment, preview) in attachments.chain(previews) {
                if !attachment.content_type.starts_with("image/")
                    || self.thumbs.contains_key(&(message.id, index))
                {
                    continue;
                }
                let thumb = match thumbnail(&attachment.bytes, self.cell, preview) {
                    Some((png, cols, rows)) => {
                        let thumb = Thumb {
                            id: self.next_id,
                            cols,
                            rows,
                        };
                        self.next_id += 1;
                        transmit(out, &thumb, &png)?;
                        Some(thumb)
                    }
                    None => None,
                };
                self.thumbs.insert((message.id, index), thumb);
            }
        }
        Ok(())
    }

    /// Frees the images sent to the terminal.
    pub fn clear(&mut self, out: &mut impl Write) -> io::Result<()> {
        for thumb in self.thumbs.drain().filter_map(|(_, thumb)| thumb) {
            write!(out, "\x1b_Ga=d,d=I,i={},q=2\x1b\\", thumb.id)?;
        }
        out.flush()
    }
}

/// The image in `bytes` at the size Signal Desktop shows it (a small image of a link
/// `preview` as an icon), as PNG, with its size in cells of `cell` pixels.
fn thumbnail(bytes: &[u8], cell: (u32, u32), preview: bool) -> Option<(Vec<u8>, u16, u16)> {
    let image = image::load_from_memory(bytes).ok()?;
    let (width, height) = size(image.width(), image.height(), preview);
    let image = image.resize(width, height, image::imageops::FilterType::CatmullRom);
    let (cw, ch) = cell;
    let max = DIACRITICS.len() as u32;
    let cols = image.width().div_ceil(cw).clamp(1, max) as u16;
    let rows = image.height().div_ceil(ch).clamp(1, max) as u16;
    let mut png = Vec::new();
    image
        .write_to(&mut io::Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    Some((png, cols, rows))
}

/// Size in pixels of a `width`×`height` image in a message, keeping its proportions: a
/// link `preview` image smaller than the minimum width both ways is an icon.
fn size(width: u32, height: u32, preview: bool) -> (u32, u32) {
    let (w, h) = (width.max(1) as f64, height.max(1) as f64);
    let small = width < WIDTH.0 && height < WIDTH.0;
    let scale = if preview && small {
        ICON as f64 / w.max(h)
    } else {
        let target = w.clamp(WIDTH.0 as f64, WIDTH.1 as f64);
        (target / w).min(MAX_HEIGHT as f64 / h)
    };
    (
        ((w * scale).round() as u32).max(1),
        ((h * scale).round() as u32).max(1),
    )
}

/// Sends `png` as the virtual placement `thumb`, in chunks, with no reply.
fn transmit(out: &mut impl Write, thumb: &Thumb, png: &[u8]) -> io::Result<()> {
    let data = STANDARD.encode(png);
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(CHUNK).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            write!(
                out,
                "\x1b_Ga=T,U=1,f=100,t=d,q=2,i={},c={},r={},m={more};",
                thumb.id, thumb.cols, thumb.rows
            )?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_placeholders_colored_by_id() {
        let thumb = Thumb {
            id: 0x010203,
            cols: 3,
            rows: 2,
        };
        let row = thumb.row(1, 2);
        assert!(row.starts_with("\x1b[38;2;1;2;3m"));
        assert_eq!(row.matches(PLACEHOLDER).count(), 2, "cut to the width");
        assert!(row.contains(&format!("{PLACEHOLDER}{}{}", DIACRITICS[1], DIACRITICS[1])));
    }

    #[test]
    fn thumbnails_fit_in_cells() {
        let image = image::RgbImage::new(1000, 500);
        let mut png = Vec::new();
        image
            .write_to(&mut io::Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let (_, cols, rows) = thumbnail(&png, (10, 20), false).unwrap();
        assert_eq!((cols, rows), (30, 8));
    }

    #[test]
    fn sizes_follow_signal_desktop() {
        assert_eq!(size(1000, 500, false), (300, 150), "wide");
        assert_eq!(size(1000, 3000, false), (150, 450), "tall");
        assert_eq!(size(100, 50, false), (200, 100), "small, enlarged");
        assert_eq!(size(600, 900, true), (300, 450), "large preview image");
        assert_eq!(
            size(150, 100, true),
            (72, 48),
            "small preview image: an icon"
        );
    }
}
