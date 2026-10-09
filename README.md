# signal-tui

Running `signal` from your terminal.

A Signal client for the terminal, linked to your phone as a secondary device (like Signal Desktop). Built on [presage](https://github.com/whisperfish/presage).

## Install

Needs Rust ≥ 1.89, `protoc` (`protobuf-compiler` package) and a C compiler.

```sh
cargo install --locked --git https://github.com/louisrubet/signal-tui
```

or, from a checkout, `cargo install --locked --path .`. It installs `signal-tui` in `~/.cargo/bin` (make sure it is in your `PATH`).

## Run

```sh
signal-tui            # or `cargo run` from a checkout
```

First run: scan the QR code from your phone (*Settings > Linked devices*).

`signal-tui --reset` forgets the link and starts over. Also remove the old device from your phone.

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
- **Store**: `~/.local/share/signal-tui/`, override with `SIGNAL_TUI_DB`. See [Security](#security).
- **presage fork**: [louisrubet/presage](https://github.com/louisrubet/presage/tree/storage-groups) adds group sync from the Storage Service.

## Security

The store holds everything this linked device needs: its identity keys and credentials, the account entropy pool (the account root secret, also the recovery key of Signal backups), session keys and messages. Anyone with a readable copy can read and send messages as you.

- **Encrypted at rest** with SQLCipher. A store left in clear by an older version is encrypted in place on the next start.
- **The key** is 32 random bytes generated on the first start, kept in the OS keyring: Secret Service on Linux (GNOME Keyring, KWallet), Keychain on macOS, Credential Manager on Windows, under the service `signal-tui`.
  To see it on Linux: `secret-tool search service signal-tui`.
- **No keyring** (headless Linux, SSH): a passphrase is asked at startup instead, or taken from `SIGNAL_TUI_PASSPHRASE`.
- **Files** are created readable by you only (Unix).
- **Reset**: `signal-tui --reset` deletes the store and its keyring key, then links again. Also remove the old device from your phone (*Settings > Linked devices*): that is what revokes it. Losing the key means resetting.
- **Limits**: a program running in your session can read the unlocked keyring. Full disk encryption protects the rest of your machine.

## Dev

```sh
cargo run --example mock_tui      # the UI with fake chats, no Signal
cargo run --example link_device   # link, then print chats as text
cargo test                        # add `-- --ignored` for the network test
```
