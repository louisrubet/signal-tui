//! The terminal interface: discussion list, discussion view, compose and forward screens.
//!
//! [`App`] holds the state and draws it; [`App::handle_key`] turns key presses into
//! [`Action`]s that the caller carries out (sending through Signal, or locally for the mock).

use std::io::{self, Write, stdout};
use std::time::{Duration, Instant};

use base64::Engine;
use crossterm::{
    cursor,
    event::{
        DisableBracketedPaste, EnableBracketedPaste, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::data::{self, Discussion, Message};
use crate::editor::Editor;
use crate::settings::Settings;
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
    /// [`App::settings`] changed: save them.
    SettingsChanged,
    /// The discussion was pinned or unpinned ([`Discussion::pinned`]): save it.
    PinToggled(usize),
    /// Set our reaction `emoji` to message `msg` of `discussion`, or remove it.
    React { discussion: usize, msg: usize, emoji: String, remove: bool },
}

enum Screen {
    /// The chats. `selected` is a discussion index (here and in `Forward`).
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
        /// `Some` while picking a reaction: the shortcode typed so far.
        reacting: Option<String>,
    },
    /// Picking the discussion to forward a message to.
    Forward {
        discussion_idx: usize,
        msg_idx: usize,
        selected: usize,
        scroll: usize,
    },
    /// The parameters, opened from the discussion list (`back_to` is its selection).
    Settings {
        selected: usize,
        back_to: usize,
    },
}

/// A parameter shown on the parameters screen: label and the setting it toggles.
type Toggle = (&'static str, fn(&mut Settings) -> &mut bool);

/// The parameters shown, in order.
const SETTINGS: [Toggle; 1] =
    [("Desktop notifications for new messages", |s| &mut s.notifications)];

struct Compose {
    editor: Editor,
    /// Message being replied to, in the current discussion.
    reply_to: Option<usize>,
}

impl Compose {
    fn new(text: &str, reply_to: Option<usize>) -> Self {
        Compose { editor: Editor::new(text), reply_to }
    }
}

/// The reactions Signal offers first, picked with 1 to 6.
const QUICK_REACTIONS: [&str; 6] =
    ["\u{2764}\u{fe0f}", "\u{1f44d}", "\u{1f44e}", "\u{1f602}", "\u{1f62e}", "\u{1f622}"];

/// Emoji whose shortcode starts with `filter` (an exact shortcode first), one per emoji.
fn reaction_matches(filter: &str) -> Vec<(&'static str, &'static str)> {
    let exact = emoji_expander::EMOJIS.get_key_value(filter).map(|(k, v)| (*k, *v));
    let mut matches: Vec<(&str, &str)> = Vec::new();
    for (shortcode, emoji) in exact.into_iter().chain(emoji_expander::search(filter)) {
        if !matches.iter().any(|(_, e)| *e == emoji) {
            matches.push((shortcode, emoji));
        }
    }
    matches
}

/// Without news, someone is no longer shown typing after this (as Signal clients do).
const TYPING_TIMEOUT: Duration = Duration::from_secs(15);

/// Someone typing in a discussion, until `until`.
struct Typing {
    discussion: usize,
    author: String,
    name: String,
    until: Instant,
}

pub struct App {
    pub discussions: Vec<Discussion>,
    pub settings: Settings,
    typing: Vec<Typing>,
    screen: Screen,
    /// One-shot feedback shown in the footer until the next key press.
    status: Option<String>,
    /// Text to put in the clipboard on the next draw.
    clipboard: Option<String>,
    /// Chats shown by a page of the chat list, as last drawn (PageUp / PageDown).
    list_page: usize,
    /// Messages to jump by a page in a discussion, from the last draw (PageUp / PageDown).
    message_page: usize,
}

impl App {
    pub fn new(discussions: Vec<Discussion>) -> Self {
        let first = order(&discussions).first().copied().unwrap_or(0);
        App {
            discussions,
            settings: Settings::default(),
            typing: Vec::new(),
            screen: Screen::List { selected: first, scroll: 0 },
            status: None,
            clipboard: None,
            list_page: 10,
            message_page: 5,
        }
    }

    pub fn set_status(&mut self, status: impl Into<String>) {
        self.status = Some(status.into());
    }

    /// Adds a discussion at the end of the list and returns its index.
    pub fn push_discussion(&mut self, discussion: Discussion) -> usize {
        self.discussions.push(discussion);
        self.discussions.len() - 1
    }

    /// `author` (named `name`) started or stopped typing in `discussion`.
    pub fn set_typing(&mut self, discussion: usize, author: &str, name: &str, started: bool, now: Instant) {
        self.typing.retain(|t| !(t.discussion == discussion && t.author == author));
        if started {
            let (author, name) = (author.to_string(), name.to_string());
            self.typing.push(Typing { discussion, author, name, until: now + TYPING_TIMEOUT });
        }
    }

    /// When the next typing indicator expires, to redraw then.
    pub fn next_typing_expiry(&self) -> Option<Instant> {
        self.typing.iter().map(|t| t.until).min()
    }

    /// Drops the typing indicators expired at `now`.
    pub fn expire_typing(&mut self, now: Instant) {
        self.typing.retain(|t| t.until > now);
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
        // Their message arrived: they are done typing it.
        if let Some(author) = &message.author {
            self.typing.retain(|t| !(t.discussion == discussion && t.author == *author));
        }
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
            self.screen = Screen::Discussion {
                discussion_idx: selected,
                selected_msg: last_msg,
                scroll: 0,
                composing: None,
                reacting: None,
            };
            return Some(Action::Opened(selected));
        }
        let discussions = &self.discussions;

        match &mut self.screen {
            Screen::List { selected, .. } => match key.code {
                KeyCode::Up => *selected = step(discussions, *selected, -1),
                KeyCode::Down => *selected = step(discussions, *selected, 1),
                KeyCode::PageUp => *selected = step(discussions, *selected, -(self.list_page as isize)),
                KeyCode::PageDown => *selected = step(discussions, *selected, self.list_page as isize),
                KeyCode::Char('P') if *selected < discussions.len() => {
                    let idx = *selected;
                    // Pinning again puts the chat at the end of the pinned list.
                    let next_rank = self.discussions.iter().filter_map(|d| d.pinned).max().map_or(0, |r| r + 1);
                    let discussion = &mut self.discussions[idx];
                    discussion.pinned = match discussion.pinned {
                        Some(_) => None,
                        None => Some(next_rank),
                    };
                    return Some(Action::PinToggled(idx));
                }
                KeyCode::Char('p') => {
                    let back_to = *selected;
                    self.screen = Screen::Settings { selected: 0, back_to };
                }
                KeyCode::Char('q') | KeyCode::Esc => return Some(Action::Quit),
                _ => {}
            },
            Screen::Discussion { discussion_idx, selected_msg, composing, reacting, .. } => {
                let messages = &discussions[*discussion_idx].messages;
                if let Some(filter) = reacting {
                    let pick = match key.code {
                        KeyCode::Esc => {
                            *reacting = None;
                            None
                        }
                        KeyCode::Char(c @ '1'..='6') if filter.is_empty() => {
                            Some(QUICK_REACTIONS[c as usize - '1' as usize])
                        }
                        KeyCode::Char(c) if c.is_alphanumeric() || "_+-".contains(c) => {
                            filter.extend(c.to_lowercase());
                            None
                        }
                        KeyCode::Backspace => {
                            filter.pop();
                            None
                        }
                        KeyCode::Enter => reaction_matches(filter).first().map(|(_, emoji)| *emoji),
                        _ => None,
                    };
                    let emoji = pick?;
                    // Picking our current reaction again removes it.
                    let remove = messages[*selected_msg].my_reaction() == Some(emoji);
                    *reacting = None;
                    return Some(Action::React {
                        discussion: *discussion_idx,
                        msg: *selected_msg,
                        emoji: emoji.to_string(),
                        remove,
                    });
                }
                if let Some(compose) = composing {
                    let editor = &mut compose.editor;
                    match key.code {
                        KeyCode::Esc => *composing = None,
                        // Shift+Enter needs the kitty keyboard protocol; Alt+Enter works everywhere.
                        KeyCode::Enter if key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                            editor.insert("\n")
                        }
                        KeyCode::Enter => {
                            let text = editor.text().trim();
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
                        KeyCode::Backspace => editor.backspace(),
                        KeyCode::Delete => editor.delete(),
                        KeyCode::Left => editor.left(),
                        KeyCode::Right => editor.right(),
                        KeyCode::Up => editor.up(),
                        KeyCode::Down => editor.down(),
                        KeyCode::Home => editor.home(),
                        KeyCode::End => editor.end(),
                        // Ctrl+letter is a shortcut, not text (Ctrl+C must not type a "c").
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            editor.insert(c.encode_utf8(&mut [0; 4]))
                        }
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
                        KeyCode::PageUp => *selected_msg = selected_msg.saturating_sub(self.message_page),
                        KeyCode::PageDown => {
                            *selected_msg = (*selected_msg + self.message_page).min(messages.len().saturating_sub(1))
                        }
                        KeyCode::Enter => *composing = Some(Compose::new("", None)),
                        KeyCode::Esc => {
                            let back_to = *discussion_idx;
                            self.screen = Screen::List { selected: back_to, scroll: 0 };
                        }
                        KeyCode::Char('q') => return Some(Action::Quit),
                        // Actions on the selected message (there is none in an empty discussion).
                        KeyCode::Char('r' | 'f' | 'c' | 'o' | 'e') if messages.is_empty() => {}
                        KeyCode::Char('e') => *reacting = Some(String::new()),
                        KeyCode::Char('r') => {
                            *composing = Some(Compose::new("", Some(*selected_msg)))
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
                        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            *composing = Some(Compose::new(c.encode_utf8(&mut [0; 4]), None))
                        }
                        _ => {}
                    }
                }
            }
            Screen::Forward { discussion_idx, msg_idx, selected, .. } => match key.code {
                KeyCode::Up => *selected = step(discussions, *selected, -1),
                KeyCode::Down => *selected = step(discussions, *selected, 1),
                KeyCode::PageUp => *selected = step(discussions, *selected, -(self.list_page as isize)),
                KeyCode::PageDown => *selected = step(discussions, *selected, self.list_page as isize),
                KeyCode::Enter => {
                    let action = Action::Forward { from: *discussion_idx, msg: *msg_idx, to: *selected };
                    let (discussion_idx, selected_msg) = (*discussion_idx, *msg_idx);
                    self.screen =
                        Screen::Discussion { discussion_idx, selected_msg, scroll: 0, composing: None, reacting: None };
                    return Some(action);
                }
                KeyCode::Esc => {
                    let (discussion_idx, selected_msg) = (*discussion_idx, *msg_idx);
                    self.screen =
                        Screen::Discussion { discussion_idx, selected_msg, scroll: 0, composing: None, reacting: None };
                }
                _ => {}
            },
            Screen::Settings { selected, back_to } => match key.code {
                KeyCode::Up => *selected = selected.saturating_sub(1),
                KeyCode::Down => *selected = (*selected + 1).min(SETTINGS.len() - 1),
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let value = (SETTINGS[*selected].1)(&mut self.settings);
                    *value = !*value;
                    return Some(Action::SettingsChanged);
                }
                KeyCode::Esc | KeyCode::Char('p') => {
                    let selected = *back_to;
                    self.screen = Screen::List { selected, scroll: 0 };
                }
                KeyCode::Char('q') => return Some(Action::Quit),
                _ => {}
            },
        }
        None
    }

    /// Pasted text (Ctrl+Shift+V, middle click…) goes into the message being typed, as is:
    /// its line breaks stay line breaks instead of sending the message. In a discussion
    /// that is not being written to, it starts a new message.
    pub fn handle_paste(&mut self, text: &str) {
        self.status = None;
        if let Screen::Discussion { composing, .. } = &mut self.screen {
            match composing {
                Some(compose) => compose.editor.insert(text),
                None => *composing = Some(Compose::new(text, None)),
            }
        }
    }

    /// Draws the current screen on a `w` x `h` terminal.
    pub fn draw(&mut self, out: &mut impl Write, w: u16, h: u16) -> io::Result<()> {
        if let Some(text) = self.clipboard.take() {
            copy_to_clipboard(out, &text)?;
        }
        let typing: Vec<usize> = self.typing.iter().map(|t| t.discussion).collect();
        self.list_page = (h as usize).saturating_sub(FOOTER).max(1);
        match &mut self.screen {
            Screen::List { selected, scroll } => draw_list(
                out,
                &self.discussions,
                &typing,
                None,
                "\u{2191}/\u{2193} select chat   Enter open   P pin/unpin   p parameters   q quit",
                *selected,
                scroll,
                w,
                h,
            ),
            Screen::Discussion { discussion_idx, selected_msg, scroll, composing, reacting } => {
                let visible = draw_discussion(
                    out,
                    &self.discussions[*discussion_idx],
                    &self.typing.iter().filter(|t| t.discussion == *discussion_idx).map(|t| t.name.as_str()).collect::<Vec<_>>(),
                    *selected_msg,
                    scroll,
                    composing.as_ref(),
                    reacting.as_deref(),
                    self.status.as_deref(),
                    w,
                    h,
                )?;
                // A page keeps one message of context.
                self.message_page = visible.saturating_sub(1).max(1);
                Ok(())
            }
            Screen::Forward { selected, scroll, .. } => draw_list(
                out,
                &self.discussions,
                &typing,
                Some("Forward to\u{2026}"),
                "\u{2191}/\u{2193} select chat   Enter forward   Esc cancel",
                *selected,
                scroll,
                w,
                h,
            ),
            Screen::Settings { selected, .. } => {
                draw_settings(out, &mut self.settings, *selected, self.status.as_deref(), w, h)
            }
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
        // Pastes arrive as one event, so their line breaks do not act as Enter.
        execute!(out, EnableBracketedPaste)?;
        Ok(TerminalGuard { enhanced_keys })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = stdout();
        let _ = execute!(out, DisableBracketedPaste);
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

/// Discussion indices in display order: pinned first (in pin order), then the others.
fn order(discussions: &[Discussion]) -> Vec<usize> {
    let (mut pinned, others): (Vec<usize>, Vec<usize>) =
        (0..discussions.len()).partition(|&i| discussions[i].pinned.is_some());
    pinned.sort_by_key(|&i| discussions[i].pinned);
    pinned.into_iter().chain(others).collect()
}

/// The discussion `delta` places away from `selected` in display order (clamped).
fn step(discussions: &[Discussion], selected: usize, delta: isize) -> usize {
    let order = order(discussions);
    let Some(pos) = order.iter().position(|&i| i == selected) else { return order.first().copied().unwrap_or(0) };
    order[pos.saturating_add_signed(delta).min(order.len() - 1)]
}

/// A line of the chat list.
enum Row<'a> {
    Title(&'a str),
    Blank,
    Chat(usize),
}

/// The list lines: one `title` section, or "Pinned" and "Chats" sections (only "Chats"
/// when nothing is pinned).
fn list_rows<'a>(discussions: &[Discussion], title: Option<&'a str>) -> Vec<Row<'a>> {
    let order = order(discussions);
    let pinned_count = discussions.iter().filter(|d| d.pinned.is_some()).count();
    let mut rows = Vec::new();
    let section = |rows: &mut Vec<Row<'a>>, title: &'a str, chats: &[usize]| {
        if !rows.is_empty() {
            rows.push(Row::Blank);
        }
        rows.push(Row::Title(title));
        rows.push(Row::Blank);
        rows.extend(chats.iter().map(|&i| Row::Chat(i)));
    };
    match title {
        Some(title) => section(&mut rows, title, &order),
        None => {
            if pinned_count > 0 {
                section(&mut rows, "Pinned", &order[..pinned_count]);
            }
            if pinned_count == 0 || pinned_count < order.len() {
                section(&mut rows, "Chats", &order[pinned_count..]);
            }
        }
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn draw_list(
    out: &mut impl Write,
    discussions: &[Discussion],
    // Discussions where someone is typing.
    typing: &[usize],
    // Single section title; `None` for the "Pinned" / "Chats" sections.
    title: Option<&str>,
    help: &str,
    selected: usize,
    scroll: &mut usize,
    w: u16,
    h: u16,
) -> io::Result<()> {
    let width = w as usize;
    let height = h as usize;
    let viewport = height.saturating_sub(FOOTER).max(1);
    let rows = list_rows(discussions, title);

    // Keep the selected chat in view, with its section title when it is the first one.
    let sel_row = rows.iter().position(|r| matches!(r, Row::Chat(i) if *i == selected)).unwrap_or(0);
    let mut top = sel_row;
    while top > 0 && !matches!(rows[top - 1], Row::Chat(_)) {
        top -= 1;
    }
    if top < *scroll {
        *scroll = top;
    }
    if sel_row >= *scroll + viewport {
        *scroll = sel_row + 1 - viewport;
    }
    *scroll = (*scroll).min(rows.len().saturating_sub(viewport));

    queue!(out, Clear(ClearType::All), cursor::Hide)?;
    for y in 0..viewport {
        let text = match rows.get(*scroll + y) {
            Some(Row::Title(title)) => ui::pad_left(title, width),
            Some(Row::Chat(idx)) => {
                let idx = *idx;
                let discussion = &discussions[idx];
                let title =
                    if typing.contains(&idx) { format!("{} ...", discussion.title) } else { discussion.title.clone() };
                // "⭐ " and the blank prefix both take three columns.
                let text = if discussion.unread {
                    format!("\u{2b50} {}", ui::pad_left(&title, width.saturating_sub(3)))
                } else {
                    format!("   {}", ui::pad_left(&title, width.saturating_sub(3)))
                };
                print_row(out, y as u16, &text, idx == selected)?;
                continue;
            }
            Some(Row::Blank) | None => " ".repeat(width),
        };
        print_row(out, y as u16, &text, false)?;
    }

    print_row(out, (height - 1) as u16, &ui::pad_left(help, width), false)?;
    Ok(())
}

fn draw_settings(
    out: &mut impl Write,
    settings: &mut Settings,
    selected: usize,
    status: Option<&str>,
    w: u16,
    h: u16,
) -> io::Result<()> {
    let width = w as usize;
    let height = h as usize;
    queue!(out, Clear(ClearType::All), cursor::Hide)?;
    print_row(out, 0, &ui::pad_left("Parameters", width), false)?;
    for (i, (label, value)) in SETTINGS.iter().enumerate() {
        let mark = if *value(settings) { "x" } else { " " };
        print_row(out, (HEADER + i) as u16, &ui::pad_left(&format!("[{mark}] {label}"), width), i == selected)?;
    }
    let help = "\u{2191}/\u{2193} select   Space/Enter toggle   Esc back   q quit";
    print_row(out, (height - 1) as u16, &ui::pad_left(status.unwrap_or(help), width), false)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn draw_discussion(
    out: &mut impl Write,
    discussion: &Discussion,
    // Names of the people typing.
    typing: &[&str],
    selected_msg: usize,
    scroll: &mut usize,
    composing: Option<&Compose>,
    // The shortcode typed so far while picking a reaction.
    reacting: Option<&str>,
    status: Option<&str>,
    w: u16,
    h: u16,
) -> io::Result<usize> {
    let width = w as usize;
    let height = h as usize;

    // Footer: the help/status line, or the compose area with one row per line of the draft
    // (at most half the screen, showing the end of the draft).
    let mut cursor_pos = None;
    let footer: Vec<String> = match composing {
        Some(Compose { editor, reply_to }) => {
            let prompt = match reply_to.and_then(|i| discussion.messages.get(i)) {
                Some(m) if m.from_me => "Reply to yourself> ".to_string(),
                Some(m) => format!("Reply to {}> ", m.sender_name),
                None => "> ".to_string(),
            };
            let indent = " ".repeat(prompt.chars().count());
            let avail = width.saturating_sub(indent.len()).max(1);
            let max_rows = (height.saturating_sub(HEADER) / 2).max(1);
            let draft: Vec<&str> = editor.text().split('\n').collect();
            let (cursor_line, cursor_col) = editor.cursor_line_col();
            // Rows shown: the end of the draft, or up to the cursor line if it is higher.
            let first = draft.len().saturating_sub(max_rows).min(cursor_line);
            // The cursor line scrolls sideways to keep the cursor visible.
            let offset = cursor_col.saturating_sub(avail - 1);
            let rows: Vec<String> = draft[first..(first + max_rows).min(draft.len())]
                .iter()
                .enumerate()
                .map(|(i, line)| {
                    let lead = if first + i == 0 { &prompt } else { &indent };
                    let skip = if first + i == cursor_line { offset } else { 0 };
                    format!("{lead}{}", line.chars().skip(skip).take(avail).collect::<String>())
                })
                .collect();
            let col = indent.len() + cursor_col - offset;
            cursor_pos = Some((col.min(width.saturating_sub(1)), cursor_line - first));
            rows
        }
        None => match reacting {
            Some("") => {
                let quick: Vec<String> =
                    QUICK_REACTIONS.iter().enumerate().map(|(i, emoji)| format!("{} {emoji}", i + 1)).collect();
                vec![format!("React: {}   or type a shortcode   Esc cancel", quick.join("  "))]
            }
            Some(filter) => {
                let matches: Vec<String> =
                    reaction_matches(filter).iter().take(8).map(|(short, emoji)| format!("{emoji} {short}")).collect();
                let matches = if matches.is_empty() { "no match".to_string() } else { matches.join("  ") };
                vec![format!("React :{filter}  \u{2192} {matches}   Enter first")]
            }
            None => {
                let help = "\u{2191}/\u{2193} navigate   r reply   e react   f forward   c copy   o open links   Enter new message   Esc back   q quit";
                vec![status.unwrap_or(help).to_string()]
            }
        },
    };
    let viewport = height.saturating_sub(HEADER + footer.len()).max(1);

    let mut lines = ui::build_discussion_lines(discussion, width);

    let mut sel_min = None;
    let mut sel_max = None;
    for (i, l) in lines.iter().enumerate() {
        if l.msg_idx == Some(selected_msg) {
            sel_min.get_or_insert(i);
            sel_max = Some(i);
        }
    }
    let sel_min = sel_min.unwrap_or(0);
    let mut sel_max = sel_max.unwrap_or(0);

    // "Name ..." under the last message while someone types, kept in view with it.
    if !typing.is_empty() {
        if !lines.is_empty() {
            lines.push(ui::RenderLine { msg_idx: None, kind: ui::LineKind::Left(String::new()) });
        }
        lines.push(ui::RenderLine { msg_idx: None, kind: ui::LineKind::Left(format!("{} ...", typing.join(", "))) });
        if selected_msg + 1 >= discussion.messages.len() {
            sel_max = lines.len() - 1;
        }
    }

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

    let mut visible_messages = std::collections::BTreeSet::new();
    for row in 0..viewport {
        let y = (HEADER + row) as u16;
        let idx = *scroll + row;
        if idx < lines.len() {
            let rl = &lines[idx];
            visible_messages.extend(rl.msg_idx);
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
    // Messages at least partly on screen.
    Ok(visible_messages.len())
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
    fn arrows_move_the_cursor_in_the_draft() {
        let mut app = opened();
        app.handle_key(key(KeyCode::Enter));
        type_text(&mut app, "ac");
        app.handle_key(key(KeyCode::Left));
        type_text(&mut app, "b");
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Delete));
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)), None);
        let send = app.handle_key(key(KeyCode::Enter));
        assert_eq!(send, Some(Action::Send { discussion: 0, text: "bc".to_string(), reply_to: None }));
    }

    #[test]
    fn paste_keeps_line_breaks_without_sending() {
        let mut app = opened();
        app.handle_paste("line 1\nline 2");
        type_text(&mut app, "!");
        let send = app.handle_key(key(KeyCode::Enter));
        assert_eq!(send, Some(Action::Send { discussion: 0, text: "line 1\nline 2!".to_string(), reply_to: None }));
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
    fn parameters_toggle_notifications() {
        let mut app = app();
        assert!(app.settings.notifications, "on by default");
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char('p')));
        assert_eq!(app.handle_key(key(KeyCode::Char(' '))), Some(Action::SettingsChanged));
        assert!(!app.settings.notifications);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(Action::Opened(1)), "back on the same discussion");
    }

    /// The screen as text (escape sequences dropped), for rendering checks.
    fn screen(app: &mut App) -> String {
        let mut out = Vec::new();
        app.draw(&mut out, 80, 30).unwrap();
        let raw = String::from_utf8(out).unwrap();
        let mut text = String::new();
        let mut chars = raw.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // CSI sequence: skip up to its final letter.
                chars.by_ref().find(|c| c.is_ascii_alphabetic());
            } else {
                text.push(c);
            }
        }
        text
    }

    #[test]
    fn typing_is_shown_until_message_stop_or_timeout() {
        let now = Instant::now();
        let mut app = app();
        app.set_typing(1, "dad", "Dad", true, now);
        assert!(screen(&mut app).contains("Family Group ..."));
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Enter));
        assert!(screen(&mut app).contains("Dad ..."));

        let message = Message { from_me: false, author: Some("dad".to_string()), ..Message::mine(1, "hi".to_string(), None, false) };
        app.push_message(1, message);
        assert!(!screen(&mut app).contains("Dad ..."), "the message replaces the indicator");

        app.set_typing(1, "mom", "Mom", true, now);
        app.set_typing(1, "mom", "Mom", false, now);
        assert_eq!(app.next_typing_expiry(), None, "stopped");

        app.set_typing(1, "mom", "Mom", true, now);
        app.expire_typing(now + TYPING_TIMEOUT);
        assert!(!screen(&mut app).contains("Mom ..."), "expired");
    }

    #[test]
    fn pinned_chats_come_first_in_their_own_list() {
        let mut app = app();
        let text = screen(&mut app);
        assert!(text.find("Pinned").unwrap() < text.find("Alice Martin").unwrap());
        assert!(text.find("Alice Martin").unwrap() < text.find("Chats").unwrap());

        // Pin Bob (index 2): it moves to the pinned list, and the selection follows it.
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.handle_key(key(KeyCode::Char('P'))), Some(Action::PinToggled(2)));
        let text = screen(&mut app);
        assert!(text.find("Bob Dupont").unwrap() < text.find("Chats").unwrap());
        // Display order is now Alice, Bob, Family Group, Chloe.
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(Action::Opened(1)));
    }

    #[test]
    fn pinning_again_goes_to_the_end_of_the_pinned_list() {
        let mut app = app();
        let pinned = |app: &mut App| {
            let text = screen(app);
            let end = text.find("Chats").unwrap();
            let mut names: Vec<(usize, &str)> = ["Alice Martin", "Family Group", "Bob Dupont", "Chloe Renard"]
                .into_iter()
                .filter_map(|n| text[..end].find(n).map(|at| (at, n)))
                .collect();
            names.sort();
            names.into_iter().map(|(_, n)| n).collect::<Vec<_>>()
        };
        // Pin Family Group: [Alice, Family]. Unpin then re-pin Alice: [Family, Alice].
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char('P')));
        assert_eq!(pinned(&mut app), ["Alice Martin", "Family Group"]);
        app.handle_key(key(KeyCode::Up));
        app.handle_key(key(KeyCode::Char('P')));
        assert_eq!(pinned(&mut app), ["Family Group"]);
        app.handle_key(key(KeyCode::Char('P')));
        assert_eq!(pinned(&mut app), ["Family Group", "Alice Martin"]);
    }

    #[test]
    fn page_keys_jump_a_page_of_chats() {
        let titles: Vec<Discussion> = (0..50).map(|i| Discussion::new(format!("Chat {i}"))).collect();
        let mut app = App::new(titles);
        let mut out = Vec::new();
        app.draw(&mut out, 80, 11).unwrap(); // 10 rows of list above the help line
        app.handle_key(key(KeyCode::PageDown));
        app.handle_key(key(KeyCode::PageDown));
        app.handle_key(key(KeyCode::PageUp));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(Action::Opened(10)));

        let mut app = App::new((0..5).map(|i| Discussion::new(format!("Chat {i}"))).collect());
        app.handle_key(key(KeyCode::PageDown));
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Some(Action::Opened(4)), "clamped to the last chat");
    }

    #[test]
    fn page_keys_jump_a_page_of_messages() {
        let mut discussion = Discussion::new("Long".to_string());
        discussion.messages = (0..40).map(|i| Message::mine(i, format!("message {i}"), None, false)).collect();
        let mut app = App::new(vec![discussion]);
        app.handle_key(key(KeyCode::Enter)); // last message (39) selected
        let mut out = Vec::new();
        app.draw(&mut out, 80, 23).unwrap(); // 20 lines of messages: 3 lines each, 7 messages
        app.handle_key(key(KeyCode::PageUp));
        // Reply to read which message is selected.
        app.handle_key(key(KeyCode::Char('r')));
        type_text(&mut app, "x");
        let send = app.handle_key(key(KeyCode::Enter));
        let Some(Action::Send { reply_to: Some(selected), .. }) = send else { panic!("{send:?}") };
        assert_eq!(selected, 39 - app.message_page);
        assert!(app.message_page >= 5, "about a screen of messages, got {}", app.message_page);
    }

    #[test]
    fn react_with_a_quick_pick_or_a_shortcode() {
        let mut app = opened();
        let last = app.discussions[0].messages.len() - 1;
        app.handle_key(key(KeyCode::Char('e')));
        assert!(screen(&mut app).contains("React:"));
        let react = app.handle_key(key(KeyCode::Char('2')));
        let thumbs_up = "\u{1f44d}".to_string();
        assert_eq!(react, Some(Action::React { discussion: 0, msg: last, emoji: thumbs_up.clone(), remove: false }));

        app.handle_key(key(KeyCode::Char('e')));
        type_text(&mut app, "rocket");
        assert!(screen(&mut app).contains("\u{1f680} rocket"));
        let react = app.handle_key(key(KeyCode::Enter));
        assert_eq!(react, Some(Action::React { discussion: 0, msg: last, emoji: "\u{1f680}".to_string(), remove: false }));

        // Our own reaction picked again: removed.
        app.discussions[0].messages[last].set_reaction("me", true, Some(&thumbs_up));
        app.handle_key(key(KeyCode::Char('e')));
        let react = app.handle_key(key(KeyCode::Char('2')));
        assert_eq!(react, Some(Action::React { discussion: 0, msg: last, emoji: thumbs_up, remove: true }));
    }

    #[test]
    fn reactions_are_shown_under_the_message() {
        let mut app = opened();
        let last = app.discussions[0].messages.len() - 1;
        let message = &mut app.discussions[0].messages[last];
        message.set_reaction("a", false, Some("\u{1f44d}"));
        message.set_reaction("b", false, Some("\u{1f44d}"));
        message.set_reaction("c", true, Some("\u{2764}\u{fe0f}"));
        assert!(screen(&mut app).contains("\u{1f44d} 2  \u{2764}\u{fe0f}"));
        app.discussions[0].messages[last].set_reaction("c", true, None);
        assert!(!screen(&mut app).contains("\u{2764}\u{fe0f}"));
    }

    #[test]
    fn without_pinned_chats_only_chats_are_listed() {
        let mut app = app();
        app.handle_key(key(KeyCode::Char('P')));
        let text = screen(&mut app);
        assert!(!text.contains("Pinned"));
        assert!(text.contains("Chats"));
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
