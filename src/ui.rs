use crate::data::Discussion;

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
    if width == 0 {
        return String::new();
    }
    if s.chars().count() > width {
        if width == 1 {
            return "…".to_string();
        }
        let mut r: String = s.chars().take(width - 1).collect();
        r.push('…');
        r
    } else {
        s.to_string()
    }
}

pub fn pad_left(s: &str, width: usize) -> String {
    let s = trunc(s, width);
    format!("{:<width$}", s, width = width)
}

pub fn pad_right(s: &str, width: usize) -> String {
    let s = trunc(s, width);
    format!("{:>width$}", s, width = width)
}

fn pad_center(s: &str, width: usize) -> String {
    let s = trunc(s, width);
    let total = width.saturating_sub(s.chars().count());
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

fn char_boundary(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len())
}

/// Greedy word-wrap. Words longer than `width` are hard-broken.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if word.chars().count() > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let mut rest = word;
            while rest.chars().count() > width {
                let at = char_boundary(rest, width);
                lines.push(rest[..at].to_string());
                rest = &rest[at..];
            }
            current = rest.to_string();
            continue;
        }
        let candidate_len = if current.is_empty() {
            word.chars().count()
        } else {
            current.chars().count() + 1 + word.chars().count()
        };
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
pub fn build_discussion_lines(discussion: &Discussion, width: usize) -> Vec<RenderLine> {
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
            out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Right(time_str) });
        } else {
            out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Left(m.sender_name.clone()) });
            for h in header {
                out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Left(h) });
            }
            for l in wrap_text(&m.text, width) {
                out.push(RenderLine { msg_idx: Some(idx), kind: LineKind::Left(l) });
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
