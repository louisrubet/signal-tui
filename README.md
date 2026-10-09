# signal-tui

Running `signal` from your terminal.

A Signal client for the terminal, linked to your phone as a secondary device (like Signal Desktop). Built on [presage](https://github.com/whisperfish/presage).

## Run

Needs Rust ≥ 1.89, `protoc` (`protobuf-compiler` package) and a C compiler.

```sh
cargo run
```

First run: scan the QR code from your phone (*Settings > Linked devices*).

`cargo run -- --reset` forgets the link and starts over. Also remove the old device from your phone.

## Keys

| Key | Action |
|---|---|
| `↑` `↓` | select chat / message |
| `Enter` | open chat, new message, send |
| `r` | reply to the selected message |
| `e` | react to the selected message (`1`–`6` or a `:shortcode:`) |
| `f` | forward the selected message |
| `c` | copy the selected message (OSC 52) |
| `o` | open the links of the selected message |
| `Del` | delete your message for everyone (within 24 h) |
| `P` | pin / unpin the selected chat |
| `p` | parameters (from the chat list) |
| `Esc` | back |
| `q` | quit |

## Good to know

- **Formatting**: `**bold**`, `*italic*`, `~strikethrough~` apply when closed (Esc right after undoes), and are sent as Signal formatting.
- **No history.** Signal doesn't send past messages to linked devices. You only see messages received since linking.
- **②** counts the messages received since you last opened a chat; opening it shows a "2 new messages" line above them.
- **Store**: `~/.local/share/signal-tui/` (holds the device keys). Override with `SIGNAL_TUI_DB`.
- **presage fork**: [louisrubet/presage](https://github.com/louisrubet/presage/tree/storage-groups) adds group sync from the Storage Service.

## Dev

```sh
cargo run --example mock_tui      # the UI with fake chats, no Signal
cargo run --example link_device   # link, then print chats as text
cargo test                        # add `-- --ignored` for the network test
```
