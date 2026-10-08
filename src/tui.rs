//! The terminal interface: discussion list, discussion view, compose and forward screens.
//!
//! [`App`] holds the state and draws it; [`App::handle_key`] turns key presses into
//! [`Action`]s that the caller carries out (sending through Signal, or locally for the mock).

use std::io::{self, Write, stdout};

use base64::Engine;
use crossterm::{
    cursor,
    event::{
        KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    },
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::data::{self, Discussion, Message};
use crate::ui;

/// What the caller has to do after a key press.
#[derive(Debug, PartialEq)]
pub enum Action {
    Quit,
    /// Discussion opened: its messages are now read.
    Opened(usize),
    /// Send `text` in `discussion`, quoting message `reply_to` of that discussion.
    Send { discussion: usize, text: String, reply_to: Option<usize> },
    /// Send the text of message `msg` of discussion `from` to discussion `to`.
    Forward { from: usize, msg: usize, to: usize },
}

enum Screen {
    List {
        selected: usize,
        scroll: usize,
    },
    Discussion {
        discussion_idx: usize,
        selected_msg: usize,
        scroll: usize,
        /// `Some` while the user is typing a new message.
        composing: Option<Compose>,
    },
    /// Picking the discussion to forward a message to.
    Forward {
        discussion_idx: usize,
        msg_idx: usize,
        selected: usize,
        scroll: usize,
    },
}

struct Compose {
    buffer: String,
    /// Message being replied to, in the current discussion.
    reply_to: Option<usize>,
}

pub struct App {
    pub discussions: Vec<Discussion>,
    screen: Screen,
    /// One-shot feedback shown in the footer until the next key press.
    status: Option<String>,
    /// Text to put in the clipboard on the next draw.
    clipboard: Option<String>,
}

impl App {
    pub fn new(discussions: Vec<Discussion>) -> Self {
        App { discussions, screen: Screen::List { selected: 0, scroll: 0 }, status: None, clipboard: None }
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = Some(status.into());
    }

    /// Adds a discussion at the end of the list and returns its index.
    pub fn push_discussion(&mut self, discussion: Discussion) -> usize {
        self.discussions.push(discussion);
        self.discussions.len() - 1
    }

    /// Whether `discussion` is the one shown (also while forwarding from it).
    pub fn is_open(&self, discussion: usize) -> bool {
        matches!(self.screen, Screen::Discussion { discussion_idx, .. } if discussion_idx == discussion)
    }

    /// Appends a message to a discussion. If that discussion is open, the selection follows
    /// the new message when it is ours or when the previous last message was selected;
    /// otherwise a received message marks it unread.
    pub fn push_message(&mut self, discussion: usize, message: Message) {
        let open = self.is_open(discussion);
        let target = &mut self.discussions[discussion];
        let was_last = target.messages.len().saturating_sub(1);
        let from_me = message.from_me;
        target.messages.push(message);
        let last = target.messages.len() - 1;
        if !open && !from_me {
            target.unread = true;
        }
        if let Screen::Discussion { discussion_idx, selected_msg, .. } = &mut self.screen
            && *discussion_idx == discussion
            && (from_me || *selected_msg == was_last)
        {
            *selected_msg = last;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        self.status = None;
        if let Screen::List { selected, .. } = self.screen
            && key.code == KeyCode::Enter
            && selected < self.discussions.len()
        {
            let last_msg = self.discussions[selected].messages.len().saturating_sub(1);
            self.discussions[selected].unread = false;
            self.screen = Screen::Discussion { discussion_idx: selected, selected_msg: last_msg, scroll: 0, composing: None };
            return Some(Action::Opened(selected));
        }
        let discussions = &self.discussions;

        match &mut self.screen {
            Screen::List { selected, .. } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if *selected + 1 < discussions.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Char('q') | KeyCode::Esc => return Some(Action::Quit),
                _ => {}
            },
            Screen::Discussion { discussion_idx, selected_msg, composing, .. } => {
                let messages = &discussions[*discussion_idx].messages;
                if let Some(compose) = composing {
                    match key.code {
                        KeyCode::Esc => *composing = None,
                        // Shift+Enter needs the kitty keyboard protocol; Alt+Enter works everywhere.
                        KeyCode::Enter if key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                            compose.buffer.push('\n')
                        }
                        KeyCode::Enter => {
                            let text = compose.buffer.trim();
                            if !text.is_empty() {
                                let action = Action::Send {
                                    discussion: *discussion_idx,
                                    text: text.to_string(),
                                    reply_to: compose.reply_to,
                                };
                                *composing = None;
                                return Some(action);
                            }
                        }
                        KeyCode::Backspace => {
                            compose.buffer.pop();
                        }
                        KeyCode::Char(c) => compose.buffer.push(c),
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Up => *selected_msg = selected_msg.saturating_sub(1),
                        KeyCode::Down => {
                            if *selected_msg + 1 < messages.len() {
                                *selected_msg += 1;
                            }
                        }
                        KeyCode::Enter => *composing = Some(Compose { buffer: String::new(), reply_to: None }),
                        KeyCode::Esc => {
                            let back_to = *discussion_idx;
                            self.screen = Screen::List { selected: back_to, scroll: 0 };
                        }
                        KeyCode::Char('q') => return Some(Action::Quit),
                        // Actions on the selected message (there is none in an empty discussion).
                        KeyCode::Char('r' | 'f' | 'c' | 'o') if messages.is_empty() => {}
                        KeyCode::Char('r') => {
                            *composing = Some(Compose { buffer: String::new(), reply_to: Some(*selected_msg) })
                        }
                        KeyCode::Char('f') => {
                            let (discussion_idx, msg_idx) = (*discussion_idx, *selected_msg);
                            self.screen = Screen::Forward { discussion_idx, msg_idx, selected: discussion_idx, scroll: 0 };
                        }
                        KeyCode::Char('c') => {
                            self.clipboard = Some(messages[*selected_msg].text.clone());
                            self.status = Some("Message copied".to_string());
                        }
                        KeyCode::Char('o') => {
                            // The quoted message is shown with the message: its links count too.
                            let message = &messages[*selected_msg];
                            let quoted = message.reply_to.and_then(|i| messages.get(i));
                            let mut links = data::links(&message.text);
                            for link in quoted.map_or(Vec::new(), |q| data::links(&q.text)) {
                                if !links.contains(&link) {
                                    links.push(link);
                                }
                            }
                            let failed = links.iter().filter(|link| open::that_detached(link).is_err()).count();
                            self.status = Some(match (links.len(), failed) {
                                (0, _) => "No link in this message".to_string(),
                                (n, 0) => format!("Opened {n} link(s)"),
                                (n, f) => format!("Could not open {f} of {n} link(s)"),
                            });
                        }
                        KeyCode::Char(c) => *composing = Some(Compose { buffer: c.to_string(), reply_to: None }),
                        _ => {}
                    }
                }
            }
            Screen::Forward { discussion_idx, msg_idx, selected, .. } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => {
                    if *selected + 1 < discussions.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let action = Action::Forward { from: *discussion_idx, msg: *msg_idx, to: *selected };
                    let (discussion_idx, selected_msg) = (*discussion_idx, *msg_idx);
                    self.screen = Screen::Discussion { discussion_idx, selected_msg, scroll: 0, composing: None };
                    return Some(action);
                }
                KeyCode::Esc => {
                    let (discussion_idx, selected_msg) = (*discussion_idx, *msg_idx);
                    self.screen = Screen::Discussion { discussion_idx, selected_msg, scroll: 0, composing: None };
                }
                _ => {}
            },
        }
        None
    }

    /// Draws the current screen on a `w` x `h` terminal.
    pub fn draw(&mut self, out: &mut impl Write, w: u16, h: u16) -> io::Result<()> {
        if let Some(text) = self.clipboard.take() {
            copy_to_clipboard(out, &text)?;
        }
        match &mut self.screen {
            Screen::List { selected, scroll } => draw_list(
                out,
                &self.discussions,
                "Discussions",
                "\u{2191}/\u{2193} select discussion   Enter open   q quit",
                *selected,
                scroll,
                w,
                h,
            ),
            Screen::Discussion { discussion_idx, selected_msg, scroll, composing } => draw_discussion(
                out,
                &self.discussions[*discussion_idx],
                *selected_msg,
                scroll,
                composing.as_ref(),
                self.status.as_deref(),
                w,
                h,
            ),
            Screen::Forward { selected, scroll, .. } => draw_list(
                out,
                &self.discussions,
                "Forward to\u{2026}",
                "\u{2191}/\u{2193} select discussion   Enter forward   Esc cancel",
                *selected,
                scroll,
                w,
                h,
            ),
        }
    }
}

/// Puts the terminal in full-screen raw mode, and restores it when dropped (also on panic).
pub struct TerminalGuard {
    enhanced_keys: bool,
}

impl TerminalGuard {
    pub fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, cursor::Hide)?;
        // Lets the terminal report Shift+Enter distinctly from Enter (kitty keyboard protocol).
        let enhanced_keys = terminal::supports_keyboard_enhancement().unwrap_or(false);
        if enhanced_keys {
            execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        }
        Ok(TerminalGuard { enhanced_keys })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = stdout();
        if self.enhanced_keys {
            let _ = execute!(out, PopKeyboardEnhancementFlags);
        }
        let _ = execute!(out, cursor::Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

/// Copies `text` through the terminal (OSC 52), which also works over SSH.
fn copy_to_clipboard(out: &mut impl Write, text: &str) -> io::Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    write!(out, "\x1b]52;c;{encoded}\x07")
}

fn print_row(out: &mut impl Write, y: u16, text: &str, selected: bool) -> io::Result<()> {
    queue!(out, cursor::MoveTo(0, y))?;
    if selected {
        queue!(out, SetAttribute(Attribute::Reverse))?;
    }
    queue!(out, Print(text))?;
    if selected {
        queue!(out, SetAttribute(Attribute::Reset))?;
    }
    Ok(())
}

const HEADER: usize = 2;
const FOOTER: usize = 1;

#[allow(clippy::too_many_arguments)]
fn draw_list(
    out: &mut impl Write,
    discussions: &[Discussion],
    title: &str,
    help: &str,
    selected: usize,
    scroll: &mut usize,
    w: u16,
    h: u16,
) -> io::Result<()> {
    let width = w as usize;
    let height = h as usize;
    let viewport = height.saturating_sub(HEADER + FOOTER).max(1);

    if selected < *scroll {
        *scroll = selected;
    }
    if selected >= *scroll + viewport {
        *scroll = selected + 1 - viewport;
    }
    let max_scroll = discussions.len().saturating_sub(viewport);
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }

    queue!(out, Clear(ClearType::All), cursor::Hide)?;
    print_row(out, 0, &ui::pad_left(title, width), false)?;
    print_row(out, 1, &" ".repeat(width), false)?;

    for row in 0..viewport {
        let y = (HEADER + row) as u16;
        let idx = *scroll + row;
        if idx < discussions.len() {
            let discussion = &discussions[idx];
            // The star takes two columns but counts as one character: pad one less.
            let text = if discussion.unread {
                format!("\u{2b50} {}", ui::pad_left(&discussion.title, width.saturating_sub(3)))
            } else {
                format!("   {}", ui::pad_left(&discussion.title, width.saturating_sub(3)))
            };
            print_row(out, y, &text, idx == selected)?;
        } else {
            print_row(out, y, &" ".repeat(width), false)?;
        }
    }

    print_row(out, (height - 1) as u16, &ui::pad_left(help, width), false)?;
    Ok(())
}

/// Last `n` characters of `s`, so the end of what is being typed stays visible.
fn tail(s: &str, n: usize) -> &str {
    let skip = s.chars().count().saturating_sub(n);
    s.char_indices().nth(skip).map_or("", |(i, _)| &s[i..])
}

#[allow(clippy::too_many_arguments)]
fn draw_discussion(
    out: &mut impl Write,
    discussion: &Discussion,
    selected_msg: usize,
    scroll: &mut usize,
    composing: Option<&Compose>,
    status: Option<&str>,
    w: u16,
    h: u16,
) -> io::Result<()> {
    let width = w as usize;
    let height = h as usize;

    // Footer: the help/status line, or the compose area with one row per line of the draft
    // (at most half the screen, showing the end of the draft).
    let mut cursor_pos = None;
    let footer: Vec<String> = match composing {
        Some(Compose { buffer, reply_to }) => {
            let prompt = match reply_to.and_then(|i| discussion.messages.get(i)) {
                Some(m) if m.from_me => "Reply to yourself> ".to_string(),
                Some(m) => format!("Reply to {}> ", m.sender_name),
                None => "> ".to_string(),
            };
            let indent = " ".repeat(prompt.chars().count());
            let avail = width.saturating_sub(indent.len());
            let max_rows = (height.saturating_sub(HEADER) / 2).max(1);
            let draft: Vec<&str> = buffer.split('\n').collect();
            let first = draft.len().saturating_sub(max_rows);
            let rows: Vec<String> = draft[first..]
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    let lead = if first + i == 0 { &prompt } else { &indent };
                    format!("{lead}{}", tail(line, avail))
                })
                .collect();
            let last = rows.last().map_or(0, |r| r.chars().count());
            cursor_pos = Some((last.min(width.saturating_sub(1)), rows.len() - 1));
            rows
        }
        None => {
            let help = "\u{2191}/\u{2193} navigate   r reply   f forward   c copy   o open links   Enter new message   Esc back   q quit";
            vec![status.unwrap_or(help).to_string()]
        }
    };
    let viewport = height.saturating_sub(HEADER + footer.len()).max(1);

    let lines = ui::build_discussion_lines(discussion, width);

    let mut sel_min = None;
    let mut sel_max = None;
    for (i, l) in lines.iter().enumerate() {
        if l.msg_idx == Some(selected_msg) {
            sel_min.get_or_insert(i);
            sel_max = Some(i);
        }
    }
    let sel_min = sel_min.unwrap_or(0);
    let sel_max = sel_max.unwrap_or(0);

    if sel_min < *scroll {
        *scroll = sel_min;
    }
    if sel_max >= *scroll + viewport {
        *scroll = sel_max + 1 - viewport;
    }
    let max_scroll = lines.len().saturating_sub(viewport);
    if *scroll > max_scroll {
        *scroll = max_scroll;
    }

    queue!(out, Clear(ClearType::All), cursor::Hide)?;
    print_row(out, 0, &ui::pad_left(&discussion.title, width), false)?;
    print_row(out, 1, &" ".repeat(width), false)?;

    for row in 0..viewport {
        let y = (HEADER + row) as u16;
        let idx = *scroll + row;
        if idx < lines.len() {
            let rl = &lines[idx];
            let text = ui::render_line_string(&rl.kind, width);
            let selected = rl.msg_idx == Some(selected_msg);
            print_row(out, y, &text, selected)?;
        } else {
            print_row(out, y, &" ".repeat(width), false)?;
        }
    }

    let footer_y = height.saturating_sub(footer.len());
    for (row, text) in footer.iter().enumerate() {
        print_row(out, (footer_y + row) as u16, &ui::pad_left(text, width), false)?;
    }
    if let Some((col, row)) = cursor_pos {
        queue!(out, cursor::MoveTo(col as u16, (footer_y + row) as u16), cursor::Show)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn app() -> App {
        App::new(data::mock_discussions())
    }

    /// Opens the first discussion (last message selected).
    fn opened() -> App {
        let mut app = app();
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(Action::Opened(0)));
        app
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            assert_eq!(app.handle_key(key(KeyCode::Char(c))), None);
        }
    }

    #[test]
    fn quit_from_list_and_discussion() {
        assert_eq!(app().handle_key(key(KeyCode::Char('q'))), Some(Action::Quit));
        assert_eq!(opened().handle_key(key(KeyCode::Char('q'))), Some(Action::Quit));
    }

    #[test]
    fn new_message_is_sent_trimmed() {
        let mut app = opened();
        app.handle_key(key(KeyCode::Enter));
        type_text(&mut app, " hello ");
        let send = app.handle_key(key(KeyCode::Enter));
        assert_eq!(send, Some(Action::Send { discussion: 0, text: "hello".to_string(), reply_to: None }));
    }

    #[test]
    fn shift_enter_adds_a_line() {
        let mut app = opened();
        app.handle_key(key(KeyCode::Enter));
        type_text(&mut app, "a");
        assert_eq!(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT)), None);
        type_text(&mut app, "b");
        let send = app.handle_key(key(KeyCode::Enter));
        assert_eq!(send, Some(Action::Send { discussion: 0, text: "a\nb".to_string(), reply_to: None }));
    }

    #[test]
    fn reply_quotes_the_selected_message() {
        let mut app = opened();
        let last = app.discussions[0].messages.len() - 1;
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Char('r')));
        type_text(&mut app, "ok");
        let send = app.handle_key(key(KeyCode::Enter));
        assert_eq!(send, Some(Action::Send { discussion: 0, text: "ok".to_string(), reply_to: Some(last - 1) }));
    }

    #[test]
    fn forward_to_the_chosen_discussion() {
        let mut app = opened();
        let last = app.discussions[0].messages.len() - 1;
        app.handle_key(key(KeyCode::Char('f')));
        app.handle_key(key(KeyCode::Down));
        let forward = app.handle_key(key(KeyCode::Enter));
        assert_eq!(forward, Some(Action::Forward { from: 0, msg: last, to: 1 }));
    }

    #[test]
    fn received_messages_mark_other_discussions_unread() {
        let mut app = opened();
        let received = || Message { from_me: false, ..Message::mine(1, "hi".to_string(), None, false) };
        app.push_message(0, received());
        app.push_message(2, received());
        app.push_message(3, Message::mine(2, "sent".to_string(), None, false));
        assert!(!app.discussions[0].unread, "open discussion stays read");
        assert!(app.discussions[2].unread);
        assert!(!app.discussions[3].unread, "own messages are not updates");
    }

    #[test]
    fn opening_a_discussion_reads_it() {
        let mut app = app();
        app.handle_key(key(KeyCode::Down));
        assert!(app.discussions[1].unread);
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(Action::Opened(1)));
        assert!(!app.discussions[1].unread);
    }

    #[test]
    fn selection_follows_pushed_messages() {
        let mut app = opened();
        app.push_message(0, Message::mine(1, "new".to_string(), None, false));
        app.handle_key(key(KeyCode::Char('r')));
        type_text(&mut app, "x");
        let last = app.discussions[0].messages.len() - 1;
        let send = app.handle_key(key(KeyCode::Enter));
        assert_eq!(send, Some(Action::Send { discussion: 0, text: "x".to_string(), reply_to: Some(last) }));
    }
}
