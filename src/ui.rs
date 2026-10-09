use unicode_width::UnicodeWidthStr;

use std::ops::Range;

use crate::data::{Discussion, Styled};

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
    /// Formatting of the line text.
    pub styles: Vec<Styled>,
}

impl RenderLine {
    pub fn new(msg_idx: Option<usize>, kind: LineKind) -> Self {
        RenderLine { msg_idx, kind, styles: Vec::new() }
    }
}

/// The parts of `styles` within `range`, moved so that `range` starts at `at`.
pub fn styles_in(styles: &[Styled], range: Range<usize>, at: usize) -> Vec<Styled> {
    styles
        .iter()
        .filter_map(|s| {
            let (start, end) = (s.start.max(range.start), s.end.min(range.end));
            (start < end).then(|| Styled { start: start - range.start + at, end: end - range.start + at, style: s.style })
        })
        .collect()
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

/// The line padded to `width` columns, with its styles moved along.
pub fn render_line(line: &RenderLine, width: usize) -> (String, Vec<Styled>) {
    let (text, rendered) = match &line.kind {
        LineKind::Left(t) => (t, pad_left(t, width)),
        LineKind::Right(t) => (t, pad_right(t, width)),
        LineKind::Center(t) => (t, pad_center(t, width)),
    };
    // Where the text starts once padded, and how much of it is kept when truncated.
    let at = rendered.len() - rendered.trim_start_matches(' ').len();
    let shown = trunc(text, width);
    let kept = if shown == *text { text.len() } else { shown.len() - '…'.len_utf8() };
    (rendered, styles_in(&line.styles, 0..kept, at))
}

/// Greedy word-wrap, keeping the line breaks of `text`. Words longer than `width` are hard-broken.
#[cfg(test)]
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    wrap_ranges(text, width).into_iter().map(|r| text[r].to_string()).collect()
}

/// [`wrap_text`] as byte ranges of `text`, so that its styles can follow.
fn wrap_ranges(text: &str, width: usize) -> Vec<Range<usize>> {
    if width == 0 {
        return std::iter::once(0..text.len()).collect();
    }
    let offset = |part: &str| part.as_ptr() as usize - text.as_ptr() as usize;
    text.lines().flat_map(|line| wrap_line(line, width).into_iter().map(move |r| r.start + offset(line)..r.end + offset(line))).collect()
}

/// One line of text wrapped, as byte ranges of it (the spaces between the words of a
/// wrapped line are kept as typed).
fn wrap_line(text: &str, width: usize) -> Vec<Range<usize>> {
    let mut lines = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for word in text.split_whitespace() {
        let start = word.as_ptr() as usize - text.as_ptr() as usize;
        let end = start + word.len();
        if width_of(word) > width {
            lines.extend(current.take());
            let mut rest = start;
            while width_of(&text[rest..end]) > width {
                // At least one char per line, even wider than `width`.
                let head = fit(&text[rest..end], width);
                let at = if head.is_empty() { text[rest..].chars().next().map_or(end - rest, char::len_utf8) } else { head.len() };
                lines.push(rest..rest + at);
                rest += at;
            }
            current = (rest < end).then_some(rest..end);
            continue;
        }
        current = match current.take() {
            Some(line) if width_of(&text[line.start..end]) <= width => Some(line.start..end),
            Some(line) => {
                lines.push(line);
                Some(start..end)
            }
            None => Some(start..end),
        };
    }
    lines.extend(current);
    if lines.is_empty() {
        lines.push(0..0);
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
            out.push(RenderLine::new(None, LineKind::Center(date.format("%A, %B %-d, %Y").to_string())));
            last_date = Some(date);
        }
        if let Some((first, count)) = new_from
            && idx == first
        {
            let label = if count == 1 { "1 new message".to_string() } else { format!("{count} new messages") };
            let rule = "\u{2500}\u{2500}\u{2500}";
            out.push(RenderLine::new(None, LineKind::Center(format!("{rule} {label} {rule}"))));
        }

        let time_str = m.timestamp.format("%H:%M").to_string();
        let text = if m.deleted { "This message was deleted" } else { m.text.as_str() };

        // "Forwarded" label and quoted message, shown above the text (truncated to one line).
        let mut header = Vec::new();
        if m.forwarded {
            header.push("Forwarded".to_string());
        }
        if let Some(quoted) = m.reply_to.and_then(|i| discussion.messages.get(i)) {
            let name = if quoted.from_me { "You" } else { &quoted.sender_name };
            let quoted_text = if quoted.deleted { "This message was deleted" } else { quoted.text.as_str() };
            header.push(format!("\u{21b3} {name}: {quoted_text}"));
        }

        if m.from_me {
            for h in header {
                out.push(RenderLine::new(Some(idx), LineKind::Right(h)));
            }
            for r in wrap_ranges(text, width) {
                let styles = styles_in(&m.styles, r.clone(), 0);
                out.push(RenderLine { styles, ..RenderLine::new(Some(idx), LineKind::Right(text[r].to_string())) });
            }
            if !m.reactions.is_empty() {
                out.push(RenderLine::new(Some(idx), LineKind::Right(m.reaction_summary())));
            }
            out.push(RenderLine::new(Some(idx), LineKind::Right(time_str)));
        } else {
            out.push(RenderLine::new(Some(idx), LineKind::Left(m.sender_name.clone())));
            for h in header {
                out.push(RenderLine::new(Some(idx), LineKind::Left(h)));
            }
            for r in wrap_ranges(text, width) {
                let styles = styles_in(&m.styles, r.clone(), 0);
                out.push(RenderLine { styles, ..RenderLine::new(Some(idx), LineKind::Left(text[r].to_string())) });
            }
            if !m.reactions.is_empty() {
                out.push(RenderLine::new(Some(idx), LineKind::Left(m.reaction_summary())));
            }
            out.push(RenderLine::new(Some(idx), LineKind::Left(time_str)));
        }

        // Blank spacer between messages for readability.
        out.push(RenderLine::new(None, LineKind::Left(String::new())));
    }

    if matches!(out.last(), Some(RenderLine { msg_idx: None, kind: LineKind::Left(s), .. }) if s.is_empty()) {
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
    fn styles_follow_wrapping_and_alignment() {
        use super::{LineKind, build_discussion_lines, render_line};
        use crate::data::{Discussion, Message, Style, Styled};
        // "aa bbbb cc", "bbbb cc" bold, wrapped at 7 columns: "aa bbbb" / "cc".
        let mut message = Message::mine(1, "aa bbbb cc".to_string(), None, false);
        message.styles = vec![Styled { start: 3, end: 10, style: Style::Bold }];
        let discussion = Discussion { messages: vec![message], ..Discussion::new("t".to_string()) };
        let lines = build_discussion_lines(&discussion, 7, None);
        let texts: Vec<_> = lines.iter().map(|l| match &l.kind {
            LineKind::Right(t) | LineKind::Left(t) | LineKind::Center(t) => t.as_str(),
        }).collect();
        assert_eq!(&texts[1..3], ["aa bbbb", "cc"]);
        assert_eq!(lines[1].styles, [Styled { start: 3, end: 7, style: Style::Bold }]);
        assert_eq!(lines[2].styles, [Styled { start: 0, end: 2, style: Style::Bold }]);
        // Right-aligned in 10 columns: "cc" starts at 8.
        let (text, styles) = render_line(&lines[2], 10);
        assert_eq!(text, "        cc");
        assert_eq!(styles, [Styled { start: 8, end: 10, style: Style::Bold }]);
    }

    #[test]
    fn wrap_keeps_line_breaks() {
        assert_eq!(wrap_text("one two\n\nthree", 20), vec!["one two", "", "three"]);
        assert_eq!(wrap_text("aaa bbb\nccc", 4), vec!["aaa", "bbb", "ccc"]);
    }
}
