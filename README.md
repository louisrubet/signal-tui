# signal-tui

Running `signal` from your terminal.

A Signal client for the terminal, linked to your phone as a secondary device (like Signal Desktop). Built on [presage](https://github.com/whisperfish/presage).

## Run

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
| `Shift+Enter` / `Alt+Enter` | new line |
| `r` | reply to the selected message |
| `f` | forward the selected message |
| `c` | copy the selected message (OSC 52) |
| `o` | open the links of the selected message |
| `Esc` | back |
| `q` | quit |

## Good to know

- **No history.** Signal doesn't send past messages to linked devices. You only see messages received since linking.
- **Store**: `~/.local/share/signal-tui/` (holds the device keys). Override with `SIGNAL_TUI_DB`.
- **presage fork**: [louisrubet/presage](https://github.com/louisrubet/presage/tree/storage-groups) adds group sync from the Storage Service.

## Dev

```sh
cargo run --example mock_tui      # the UI with fake chats, no Signal
cargo run --example link_device   # link, then print chats as text
cargo test                        # add `-- --ignored` for the network test
```
