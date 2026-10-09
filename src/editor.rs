//! Multi-line text being typed, with a cursor and formatting.
//!
//! `**bold**`, `*italic*` and `~strikethrough~` are applied when their closing marker is
//! typed: the markers go and the text keeps the style ([`Editor::styles`]).

use std::ops::Range;

use crate::data::{Style, Styled};

#[derive(Default)]
pub struct Editor {
    text: String,
    /// Byte index in `text`, always on a char boundary.
    cursor: usize,
    /// Formatting of `text`.
    styles: Vec<Styled>,
    /// The state before the last formatting, kept while nothing changed since.
    undo: Option<Undo>,
}

struct Undo {
    text: String,
    cursor: usize,
    styles: Vec<Styled>,
    /// What the formatting gave, to check nothing changed since.
    after: (String, usize),
}

/// Formatting markers, longest first so that `**` is not taken for two `*`.
const MARKERS: [(&str, Style); 3] = [("**", Style::Bold), ("*", Style::Italic), ("~", Style::Strikethrough)];

impl Editor {
    /// An editor holding `text`, cursor at the end.
    pub fn new(text: &str) -> Self {
        let text = normalize_newlines(text);
        Editor { cursor: text.len(), text, ..Default::default() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn styles(&self) -> &[Styled] {
        &self.styles
    }

    /// Replaces `range` of the text by `with`, keeping the styles on the same characters:
    /// text inserted inside a styled range takes its style, not at its edges.
    fn splice(&mut self, range: Range<usize>, with: &str) {
        let (at, removed, added) = (range.start, range.len(), with.len());
        self.text.replace_range(range, with);
        // Removal: positions inside the removed part move to its start.
        let removal = |p: usize| if p <= at { p } else if p < at + removed { at } else { p - removed };
        for styled in &mut self.styles {
            let (start, end) = (removal(styled.start), removal(styled.end));
            // Insertion at `at`: a range starting there moves, one ending there does not grow.
            styled.start = if start >= at { start + added } else { start };
            styled.end = if end > at || (end == at && start >= at) { end + added } else { end };
        }
        self.styles.retain(|s| s.start < s.end);
    }

    /// Undoes the formatting just applied (the markers come back as plain text), if nothing
    /// changed since. Returns whether it did.
    pub fn undo_format(&mut self) -> bool {
        let Some(undo) = self.undo.take() else { return false };
        if undo.after != (self.text.clone(), self.cursor) {
            return false;
        }
        (self.text, self.cursor, self.styles) = (undo.text, undo.cursor, undo.styles);
        true
    }

    /// Applies the formatting whose closing marker was just typed, if any.
    fn format(&mut self) {
        let before = &self.text[..self.cursor];
        for (marker, style) in MARKERS {
            let Some(body) = before.strip_suffix(marker) else { continue };
            // A single `*` right after another one is the end of a `**`.
            if marker == "*" && body.ends_with('*') {
                continue;
            }
            let Some(open) = body.rfind(marker) else { continue };
            let content = &body[open + marker.len()..];
            // The opening marker starts a word: not inside one (`a*b*`), nor part of `**`.
            let starts_word = body[..open].chars().next_back().is_none_or(|c| !c.is_alphanumeric() && c != '*');
            let valid = !content.is_empty()
                && !content.contains('\n')
                && !content.starts_with(char::is_whitespace)
                && !content.ends_with(char::is_whitespace)
                && !(marker == "*" && content.starts_with('*'))
                && starts_word;
            if !valid {
                return;
            }
            let undo = (self.text.clone(), self.cursor, self.styles.clone());
            let content_len = content.len();
            self.splice(self.cursor - marker.len()..self.cursor, "");
            self.splice(open..open + marker.len(), "");
            self.styles.push(Styled { start: open, end: open + content_len, style });
            self.cursor = open + content_len;
            let after = (self.text.clone(), self.cursor);
            self.undo = Some(Undo { text: undo.0, cursor: undo.1, styles: undo.2, after });
            return;
        }
    }

    /// Byte index of the cursor in [`Editor::text`].
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replaces the text from byte `start` up to the cursor by `with`.
    pub fn replace_to_cursor(&mut self, start: usize, with: &str) {
        self.splice(start..self.cursor, with);
        self.cursor = start + with.len();
    }

    /// The text of the cursor line, before the cursor.
    pub fn line_before_cursor(&self) -> &str {
        &self.text[self.line_start(self.cursor)..self.cursor]
    }

    /// Line and column (in chars) of the cursor.
    pub fn cursor_line_col(&self) -> (usize, usize) {
        let before = &self.text[..self.cursor];
        let line = before.matches('\n').count();
        let col = before.rsplit('\n').next().unwrap_or("").chars().count();
        (line, col)
    }

    pub fn insert(&mut self, s: &str) {
        let s = normalize_newlines(s);
        self.splice(self.cursor..self.cursor, &s);
        self.cursor += s.len();
        // Typing the closing colon of a known `:shortcode:` turns it into the emoji.
        if s == ":" {
            self.expand_shortcode();
        }
        if s == "*" || s == "~" {
            self.format();
        }
    }

    /// Replaces the `:shortcode:` that ends at the cursor by its emoji, if it is known.
    fn expand_shortcode(&mut self) {
        let Some(body) = self.text[..self.cursor].strip_suffix(':') else { return };
        let Some(start) = body.rfind(':') else { return };
        let shortcode = &body[start + 1..];
        let valid = !shortcode.is_empty() && shortcode.chars().all(|c| c.is_ascii_alphanumeric() || "_+-".contains(c));
        if !valid {
            return;
        }
        if let Some(emoji) = emoji_expander::lookup(shortcode) {
            self.splice(start..self.cursor, emoji);
            self.cursor = start + emoji.len();
        }
    }

    pub fn backspace(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.splice(prev..self.cursor, "");
            self.cursor = prev;
        }
    }

    pub fn delete(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.splice(self.cursor..next, "");
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.prev_boundary().unwrap_or(self.cursor);
    }

    pub fn right(&mut self) {
        self.cursor = self.next_boundary().unwrap_or(self.cursor);
    }

    /// Start of the previous word (words are letters, digits and `_`).
    pub fn word_left(&mut self) {
        let before: Vec<(usize, char)> = self.text[..self.cursor].char_indices().collect();
        let mut i = before.len();
        while i > 0 && !is_word(before[i - 1].1) {
            i -= 1;
        }
        while i > 0 && is_word(before[i - 1].1) {
            i -= 1;
        }
        self.cursor = before.get(i).map_or(self.cursor, |(at, _)| *at);
    }

    /// End of the next word.
    pub fn word_right(&mut self) {
        let mut rest = self.text[self.cursor..].char_indices().peekable();
        while rest.next_if(|(_, c)| !is_word(*c)).is_some() {}
        while rest.next_if(|(_, c)| is_word(*c)).is_some() {}
        self.cursor += rest.peek().map_or(self.text.len() - self.cursor, |(at, _)| *at);
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start(self.cursor);
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end(self.cursor);
    }

    /// Previous line, same column if possible; start of the text on the first line.
    pub fn up(&mut self) {
        let start = self.line_start(self.cursor);
        if start == 0 {
            self.cursor = 0;
            return;
        }
        let (_, col) = self.cursor_line_col();
        self.cursor = self.at_col(self.line_start(start - 1), col);
    }

    /// Next line, same column if possible; end of the text on the last line.
    pub fn down(&mut self) {
        let end = self.line_end(self.cursor);
        if end == self.text.len() {
            self.cursor = end;
            return;
        }
        let (_, col) = self.cursor_line_col();
        self.cursor = self.at_col(end + 1, col);
    }

    fn prev_boundary(&self) -> Option<usize> {
        self.text[..self.cursor].char_indices().next_back().map(|(i, _)| i)
    }

    fn next_boundary(&self) -> Option<usize> {
        self.text[self.cursor..].chars().next().map(|c| self.cursor + c.len_utf8())
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self, at: usize) -> usize {
        self.text[at..].find('\n').map_or(self.text.len(), |i| at + i)
    }

    /// Byte index of column `col` in the line starting at `start`, clamped to its end.
    fn at_col(&self, start: usize, col: usize) -> usize {
        let line = &self.text[start..self.line_end(start)];
        start + line.char_indices().nth(col).map_or(line.len(), |(i, _)| i)
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Pasted text can come with `\r\n` or `\r` line ends.
fn normalize_newlines(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

#[cfg(test)]
mod tests {
    use super::Editor;
    use crate::data::{Style, Styled};

    fn typed(text: &str) -> Editor {
        let mut e = Editor::default();
        for c in text.chars() {
            e.insert(c.encode_utf8(&mut [0; 4]));
        }
        e
    }

    fn styled(start: usize, end: usize, style: Style) -> Styled {
        Styled { start, end, style }
    }

    #[test]
    fn markers_format_when_closed() {
        let e = typed("a **b** *c* ~d~ e");
        assert_eq!(e.text(), "a b c d e");
        assert_eq!(e.styles(), [styled(2, 3, Style::Bold), styled(4, 5, Style::Italic), styled(6, 7, Style::Strikethrough)]);
    }

    #[test]
    fn esc_undoes_the_formatting_just_applied() {
        let mut e = typed("Salut **tout**");
        assert_eq!(e.text(), "Salut tout");
        assert!(e.undo_format());
        assert_eq!(e.text(), "Salut **tout**");
        assert!(e.styles().is_empty());
        assert!(!e.undo_format(), "once");

        let mut e = typed("*a*");
        e.insert(" ");
        assert!(!e.undo_format(), "not after another change");
        assert_eq!(e.text(), "a ");
    }

    #[test]
    fn markers_need_words() {
        for plain in ["2 * 3 * 4", "a*b*", "* x*", "** **", "~ ~", "*a\nb*"] {
            assert_eq!(typed(plain).text(), plain);
        }
    }

    #[test]
    fn styles_follow_edits() {
        let mut e = typed("**bold** x");
        e.home();
        e.insert(">> ");
        assert_eq!(e.styles(), [styled(3, 7, Style::Bold)], "moved by text before");
        e.right();
        e.right();
        e.insert("o");
        assert_eq!(e.text(), ">> booold x".replacen("oo", "o", 1));
        assert_eq!(e.styles(), [styled(3, 8, Style::Bold)], "grows with text inside");
        e.end();
        e.insert("!");
        assert_eq!(e.styles(), [styled(3, 8, Style::Bold)], "not with text after");
    }

    #[test]
    fn inserts_at_the_cursor() {
        let mut e = Editor::new("hllo");
        e.home();
        e.right();
        e.insert("e");
        assert_eq!(e.text(), "hello");
        assert_eq!(e.cursor_line_col(), (0, 2));
    }

    #[test]
    fn backspace_and_delete_around_multibyte_chars() {
        let mut e = Editor::new("aéb");
        e.left();
        e.backspace();
        assert_eq!(e.text(), "ab");
        e.left();
        e.delete();
        assert_eq!(e.text(), "b");
    }

    #[test]
    fn up_and_down_keep_the_column() {
        let mut e = Editor::new("first line\nab\nthird line");
        assert_eq!(e.cursor_line_col(), (2, 10));
        e.up();
        assert_eq!(e.cursor_line_col(), (1, 2), "clamped to the short line");
        e.up();
        assert_eq!(e.cursor_line_col(), (0, 2));
        e.up();
        assert_eq!(e.cursor_line_col(), (0, 0));
        e.down();
        e.down();
        e.down();
        assert_eq!(e.cursor_line_col(), (2, 10));
    }

    #[test]
    fn typed_shortcodes_become_emoji() {
        let mut e = Editor::default();
        for c in "go :rocket: at 10:30: :nope:".chars() {
            e.insert(c.encode_utf8(&mut [0; 4]));
        }
        assert_eq!(e.text(), "go \u{1f680} at 10:30: :nope:");
        assert_eq!(e.cursor_line_col(), (0, e.text().chars().count()));
    }

    #[test]
    fn word_moves() {
        let mut e = Editor::new("Bonjour, le monde\nété 2026");
        e.word_left();
        assert_eq!(e.cursor_line_col(), (1, 4), "start of 2026");
        e.word_left();
        assert_eq!(e.cursor_line_col(), (1, 0), "start of été, over the space");
        e.word_left();
        assert_eq!(e.cursor_line_col(), (0, 12), "start of monde, over the line break");
        e.word_left();
        e.word_left();
        assert_eq!(e.cursor_line_col(), (0, 0), "over the comma");
        e.word_left();
        assert_eq!(e.cursor_line_col(), (0, 0), "stays at the start");
        e.word_right();
        assert_eq!(e.cursor_line_col(), (0, 7), "end of Bonjour");
        e.word_right();
        assert_eq!(e.cursor_line_col(), (0, 11), "end of le");
        e.word_right();
        e.word_right();
        assert_eq!(e.cursor_line_col(), (1, 3), "end of été");
        e.word_right();
        e.word_right();
        assert_eq!(e.cursor_line_col(), (1, 8), "stays at the end");
    }

    #[test]
    fn pasted_line_ends_are_normalized() {
        let mut e = Editor::default();
        e.insert("a\r\nb\rc");
        assert_eq!(e.text(), "a\nb\nc");
    }
}
