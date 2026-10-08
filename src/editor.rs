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
    fn pasted_line_ends_are_normalized() {
        let mut e = Editor::default();
        e.insert("a\r\nb\rc");
        assert_eq!(e.text(), "a\nb\nc");
    }
}
