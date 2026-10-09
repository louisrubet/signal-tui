//! Multi-line text being typed, with a cursor.

#[derive(Default)]
pub struct Editor {
    text: String,
    /// Byte index in `text`, always on a char boundary.
    cursor: usize,
}

impl Editor {
    /// An editor holding `text`, cursor at the end.
    pub fn new(text: &str) -> Self {
        let text = normalize_newlines(text);
        Editor { cursor: text.len(), text }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Byte index of the cursor in [`Editor::text`].
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replaces the text from byte `start` up to the cursor by `with`.
    pub fn replace_to_cursor(&mut self, start: usize, with: &str) {
        self.text.replace_range(start..self.cursor, with);
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
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
        // Typing the closing colon of a known `:shortcode:` turns it into the emoji.
        if s == ":" {
            self.expand_shortcode();
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
            self.text.replace_range(start..self.cursor, emoji);
            self.cursor = start + emoji.len();
        }
    }

    pub fn backspace(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.text.replace_range(prev..self.cursor, "");
            self.cursor = prev;
        }
    }

    pub fn delete(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.text.replace_range(self.cursor..next, "");
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
