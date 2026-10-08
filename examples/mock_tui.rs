use std::io::{self, stdout, Write};

use base64::Engine;

use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};

use mysignalcli::{data::{self, Discussion}, ui};

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

fn main() -> io::Result<()> {
    let mut discussions = data::mock_discussions();

    terminal::enable_raw_mode()?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen, cursor::Hide)?;

    let result = run(&mut out, &mut discussions);

    execute!(out, cursor::Show, LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    result
}

fn run(out: &mut impl Write, discussions: &mut Vec<Discussion>) -> io::Result<()> {
    let mut screen = Screen::List { selected: 0, scroll: 0 };
    // One-shot feedback shown in the footer until the next key press.
    let mut status: Option<String> = None;

    loop {
        let (w, h) = terminal::size()?;

        match &mut screen {
            Screen::List { selected, scroll } => draw_list(
                out,
                discussions,
                "Discussions",
                "\u{2191}/\u{2193} select discussion   Enter open   q quit",
                *selected,
                scroll,
                w,
                h,
            )?,
            Screen::Discussion { discussion_idx, selected_msg, scroll, composing } => draw_discussion(
                out,
                &discussions[*discussion_idx],
                *selected_msg,
                scroll,
                composing.as_ref(),
                status.as_deref(),
                w,
                h,
            )?,
            Screen::Forward { selected, scroll, .. } => draw_list(
                out,
                discussions,
                "Forward to\u{2026}",
                "\u{2191}/\u{2193} select discussion   Enter forward   Esc cancel",
                *selected,
                scroll,
                w,
                h,
            )?,
        }
        out.flush()?;

        let Event::Key(key) = event::read()? else { continue };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        status = None;

        match &mut screen {
            Screen::List { selected, .. } => match key.code {
                KeyCode::Up => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Down => {
                    if *selected + 1 < discussions.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let discussion_idx = *selected;
                    let last_msg = discussions[discussion_idx].messages.len().saturating_sub(1);
                    screen = Screen::Discussion { discussion_idx, selected_msg: last_msg, scroll: 0, composing: None };
                }
                KeyCode::Char('q') | KeyCode::Esc => break,
                _ => {}
            },
            Screen::Discussion { discussion_idx, selected_msg, composing, .. } => {
                if let Some(compose) = composing {
                    match key.code {
                        KeyCode::Esc => *composing = None,
                        KeyCode::Enter => {
                            let text = compose.buffer.trim();
                            if !text.is_empty() {
                                discussions[*discussion_idx].messages.push(data::Message {
                                    from_me: true,
                                    sender_name: String::new(),
                                    text: text.to_string(),
                                    timestamp: chrono::Local::now().naive_local(),
                                    reply_to: compose.reply_to,
                                    forwarded: false,
                                });
                                *selected_msg = discussions[*discussion_idx].messages.len() - 1;
                                *composing = None;
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
                        KeyCode::Up => {
                            if *selected_msg > 0 {
                                *selected_msg -= 1;
                            }
                        }
                        KeyCode::Down => {
                            let max = discussions[*discussion_idx].messages.len().saturating_sub(1);
                            if *selected_msg < max {
                                *selected_msg += 1;
                            }
                        }
                        KeyCode::Enter => *composing = Some(Compose { buffer: String::new(), reply_to: None }),
                        KeyCode::Esc => {
                            let back_to = *discussion_idx;
                            screen = Screen::List { selected: back_to, scroll: 0 };
                        }
                        KeyCode::Char('q') => break,
                        // Actions on the selected message (there is none in an empty discussion).
                        KeyCode::Char('r' | 'f' | 'c' | 'o') if discussions[*discussion_idx].messages.is_empty() => {}
                        KeyCode::Char('r') => {
                            *composing = Some(Compose { buffer: String::new(), reply_to: Some(*selected_msg) })
                        }
                        KeyCode::Char('f') => {
                            let (discussion_idx, msg_idx) = (*discussion_idx, *selected_msg);
                            screen = Screen::Forward { discussion_idx, msg_idx, selected: discussion_idx, scroll: 0 };
                        }
                        KeyCode::Char('c') => {
                            copy_to_clipboard(out, &discussions[*discussion_idx].messages[*selected_msg].text)?;
                            status = Some("Message copied".to_string());
                        }
                        KeyCode::Char('o') => {
                            let links = data::links(&discussions[*discussion_idx].messages[*selected_msg].text);
                            let failed = links.iter().filter(|link| open::that_detached(link).is_err()).count();
                            status = Some(match (links.len(), failed) {
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
                KeyCode::Up => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Down => {
                    if *selected + 1 < discussions.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let (source, msg_idx, target) = (*discussion_idx, *msg_idx, *selected);
                    let text = discussions[source].messages[msg_idx].text.clone();
                    discussions[target].messages.push(data::Message {
                        from_me: true,
                        sender_name: String::new(),
                        text,
                        timestamp: chrono::Local::now().naive_local(),
                        reply_to: None,
                        forwarded: true,
                    });
                    status = Some(format!("Forwarded to {}", discussions[target].title));
                    screen = Screen::Discussion { discussion_idx: source, selected_msg: msg_idx, scroll: 0, composing: None };
                }
                KeyCode::Esc => {
                    let (discussion_idx, selected_msg) = (*discussion_idx, *msg_idx);
                    screen = Screen::Discussion { discussion_idx, selected_msg, scroll: 0, composing: None };
                }
                _ => {}
            },
        }
    }

    Ok(())
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
            let text = ui::pad_left(&discussions[idx].title, width);
            print_row(out, y, &text, idx == selected)?;
        } else {
            print_row(out, y, &" ".repeat(width), false)?;
        }
    }

    print_row(
        out,
        (height - 1) as u16,
        &ui::pad_left(help, width),
        false,
    )?;
    Ok(())
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
    let viewport = height.saturating_sub(HEADER + FOOTER).max(1);

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

    let bottom_y = (height - 1) as u16;
    match composing {
        Some(Compose { buffer, reply_to }) => {
            let prompt = match reply_to.and_then(|i| discussion.messages.get(i)) {
                Some(m) if m.from_me => "Reply to yourself> ".to_string(),
                Some(m) => format!("Reply to {}> ", m.sender_name),
                None => "> ".to_string(),
            };
            let avail = width.saturating_sub(prompt.chars().count());
            let shown: String = if buffer.chars().count() <= avail {
                buffer.to_string()
            } else {
                buffer.chars().rev().take(avail).collect::<Vec<_>>().into_iter().rev().collect()
            };
            let line = format!("{prompt}{shown}");
            let cursor_col = line.chars().count().min(width.saturating_sub(1)) as u16;
            print_row(out, bottom_y, &ui::pad_left(&line, width), false)?;
            queue!(out, cursor::MoveTo(cursor_col, bottom_y), cursor::Show)?;
        }
        None => {
            let help = "\u{2191}/\u{2193} navigate   r reply   f forward   c copy   o open links   Enter new message   Esc back   q quit";
            print_row(out, bottom_y, &ui::pad_left(status.unwrap_or(help), width), false)?;
        }
    }
    Ok(())
}
