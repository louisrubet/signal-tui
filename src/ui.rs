use unicode_width::UnicodeWidthStr;

use crate::data::Discussion;

/// Columns `s` takes on screen (an emoji takes two).
pub fn width_of(s: &str) -> usize {
    s.width()
}

/// The part of `s` starting `skip` columns in, at most `width` columns wide.
pub fn columns(s: &str, skip: usize, width: usize) -> &str {
    let start = s.char_indices().map(|(i, _)| i).find(|&i| width_of(&s[..i]) >= skip).unwrap_or(s.len());
    fit(&s[start..], width)
}

/// Longest prefix of `s` that fits in `width` columns.
fn fit(s: &str, width: usize) -> &str {
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let next = i + c.len_utf8();
        if width_of(&s[..next]) > width {
            break;
        }
        end = next;
    }
    &s[..end]
}

#[derive(Debug)]
pub enum LineKind {
    Left(String),
    Right(String),
    Center(String),
}

pub struct RenderLine {
    /// Index of the message this line belongs to, if any (blank/date lines have None).
    pub msg_idx: Option<usize>,
    pub kind: LineKind,
}

pub fn trunc(s: &str, width: usize) -> String {
    if width_of(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    format!("{}…", fit(s, width - 1))
}

pub fn pad_left(s: &str, width: usize) -> String {
    let s = trunc(s, width);
    let pad = width.saturating_sub(width_of(&s));
    format!("{s}{}", " ".repeat(pad))
}

pub fn pad_right(s: &str, width: usize) -> String {
    let s = trunc(s, width);
    let pad = width.saturating_sub(width_of(&s));
    format!("{}{s}", " ".repeat(pad))
}

fn pad_center(s: &str, width: usize) -> String {
    let s = trunc(s, width);
    let total = width.saturating_sub(width_of(&s));
    let left = total / 2;
    let right = total - left;
    format!("{}{}{}", " ".repeat(left), s, " ".repeat(right))
}

pub fn render_line_string(kind: &LineKind, width: usize) -> String {
    match kind {
        LineKind::Left(t) => pad_left(t, width),
        LineKind::Right(t) => pad_right(t, width),
        LineKind::Center(t) => pad_center(t, width),
    }
}

/// Greedy word-wrap, keeping the line breaks of `text`. Words longer than `width` are hard-broken.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    text.lines().flat_map(|line| wrap_line(line, width)).collect()
}

fn wrap_line(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if width_of(word) > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let mut rest = word;
            while width_of(rest) > width {
                // At least one char per line, even wider than `width`.
                let head = fit(rest, width);
                let at = if head.is_empty() { rest.chars().next().map_or(rest.len(), char::len_utf8) } else { head.len() };
                lines.push(rest[..at].to_string());
                rest = &rest[at..];
            }
            current = rest.to_string();
            continue;
        }
        let candidate_len =
            if current.is_empty() { width_of(word) } else { width_of(&current) + 1 + width_of(word) };
        if candidate_len > width {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Builds the flat list of screen lines for a discussion: date separators,
/// sender name / message text / time lines, and blank spacers between messages.
/// `new_from` (index, count) puts a "N new messages" line above the first new message.
pub fn build_discussion_lines(discussion: &Discussion, width: usize, new_from: Option<(usize, usize)>) -> Vec<RenderLine> {
    let mut out = Vec::new();
    let mut last_date: Option<chrono::NaiveDate> = None;

    for (idx, m) in discussion.messages.iter().enumerate() {
        let date = m.timestamp.date();
        if last_date != Some(date) {
            out.push(RenderLine {
                msg_idx: None,
                kind: LineKind::Center(date.format("%A, %B %-d, %Y").to_string()),
            });
            last_date = Some(date);
        }
        if let Some((first, count)) = new_from
            && idx == first
        {
            let label = if count == 1 { "1 new message".to_string() } else { format!("{count} new messages") };
            out.push(RenderLine {
                msg_idx: None,
                kind: LineKind::Center(format!("\u{2500}\u{2500}\u{2500} {label} \u{2500}\u{2500}\u{2500}")),
            });
        }

        let time_str = m.timestamp.format("%H:%M").to_string();

        // "Forwarded" label and quoted message, shown above the text (truncated to one line).
        let mut header = Vec::new();
        if m.forwarded {
            header.push("Forwarded".to_string());
        }
        if let Some(quoted) = m.reply_to.and_then(|i| discussion.messages.get(i)) {
            let name = if quoted.from_me { "You" } else { &quoted.sender_name };
            header.push(format!("\u{21b3} {name}: {}", quoted.text));
        }

        if m.from_me {
            for h in header {
                out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Right(h) });
            }
            for l in wrap_text(&m.text, width) {
                out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Right(l) });
            }
            if !m.reactions.is_empty() {
                out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Right(m.reaction_summary()) });
            }
            out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Right(time_str) });
        } else {
            out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Left(m.sender_name.clone()) });
            for h in header {
                out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Left(h) });
            }
            for l in wrap_text(&m.text, width) {
                out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Left(l) });
            }
            if !m.reactions.is_empty() {
                out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Left(m.reaction_summary()) });
            }
            out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Left(time_str) });
        }

        // Blank spacer between messages for readability.
        out.push(RenderLine { msg_idx: None, kind: LineKind::Left(String::new()) });
    }

    if matches!(out.last(), Some(RenderLine { msg_idx: None, kind: LineKind::Left(s) }) if s.is_empty()) {
        out.pop();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::wrap_text;

    #[test]
    fn emoji_take_two_columns() {
        use super::{pad_left, pad_right, trunc, width_of};
        assert_eq!(width_of("\u{1f44d} 2"), 4);
        assert_eq!(pad_right("\u{1f44d}", 4), "  \u{1f44d}");
        assert_eq!(width_of(&pad_left("\u{2764}\u{fe0f} 2  \u{1f44d}", 12)), 12);
        assert_eq!(trunc("\u{1f44d}\u{1f44d}\u{1f44d}", 4), "\u{1f44d}…");
    }

    #[test]
    fn wrap_keeps_line_breaks() {
        assert_eq!(wrap_text("one two\n\nthree", 20), vec!["one two", "", "three"]);
        assert_eq!(wrap_text("aaa bbb\nccc", 4), vec!["aaa", "bbb", "ccc"]);
    }
}
