# signal-tui

Running `signal` from your terminal.

A Signal client for the terminal, linked to your phone as a secondary device (like Signal Desktop). Built on [presage](https://github.com/whisperfish/presage) and [chat-tui](https://github.com/louisrubet/chat-tui), the interface it shares with google-messages-tui.

- **Images** shown as thumbnails, pasted with `Ctrl+V`, opened full size with `o`
- **Link previews** with title, description and image, as Signal sends them
- **Emojis**: reactions, fuzzy `:shortcode` picker (fzf style)
- **Replies, forwards, deletion**, `**bold**` `*italic*` `~strikethrough~`
- **Pinned chats**, unread counts, desktop notifications

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

`signal-tui --version` prints the release version. On an exact tag it prints the tag, for example `v1.0.0`; after the latest tag it prints `<tag>+<5-char-sha>`. `signal-tui --help` lists the options.

## Uninstall

```sh
signal-tui --uninstall        # store key in the keyring, ~/.local/share/signal-tui
cargo uninstall signal-tui    # the program
```

`--uninstall` asks for confirmation. Also remove the device from your phone (*Settings > Linked devices*).

## Terminal

Any terminal works. For thumbnails, use one that speaks the [kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/) **with Unicode placeholders**:

| Terminal | Linux | macOS | Windows |
|---|:-:|:-:|:-:|
| [Ghostty](https://ghostty.org) (recommended) | ✓ | ✓ | |
| [kitty](https://sw.kovidgoyal.net/kitty/) | ✓ | ✓ | |
| [Rio](https://rioterm.com) | ✓ | ✓ | ✓ |

On Windows, Rio is the only native option; kitty also runs under WSL with WSLg.

Elsewhere (WezTerm, Konsole, iTerm2, Warp, Windows Terminal, Alacritty, inside tmux…) everything works but graphics are reduced: images show as `[image #1]` and open full size with `o`, link previews show without their image.

## Keys

| Key | Action |
|---|---|
| `↑` `↓` | select chat / message (in a chat, `↑` starts selecting) |
| `Enter` | open chat, new message, send |
| `r` | reply to the selected message |
| `e` | react to the selected message (`1`–`6` or a `:shortcode:`) |
| `f` | forward the selected message |
| `c` | copy the selected message (OSC 52) |
| `o` | open the links and images of the selected message |
| `Ctrl+V` | paste the image in the clipboard |
| `Del` | delete your message for everyone (within 24 h) |
| `P` | pin / unpin the selected chat |
| `p` | parameters (from the chat list) |
| `Esc` | unselect, back |
| `q` | quit |

## Good to know

- **Formatting**: `**bold**`, `*italic*`, `~strikethrough~` apply when closed (Esc right after undoes), and are sent as Signal formatting.
- **Downloads**: images are fetched when they arrive or when a chat is opened; videos and files wait for `o`.
- **No history.** Signal doesn't send past messages to linked devices. You only see messages received since linking.
- **②** counts the messages received since you last opened a chat; opening it shows a "2 new messages" line above them.
- **Store**: `~/.local/share/signal-tui/`, override with `SIGNAL_TUI_DB`. See [Security](#security).
- **Interface**: [chat-tui](https://github.com/louisrubet/chat-tui), pinned by tag in `Cargo.toml`; its demo with fake chats is `cargo run --example mock_tui` there.
- **presage fork**: [louisrubet/presage](https://github.com/louisrubet/presage/tree/sync-groups-from-storage-service) adds group sync from the Storage Service.

## Security

The store holds everything this linked device needs: its identity keys and credentials, the account entropy pool (the account root secret, also the recovery key of Signal backups), session keys and messages. Anyone with a readable copy can read and send messages as you.

- **Encrypted at rest** with SQLCipher. A store left in clear by an older version is encrypted in place on the next start.
- **The key** is 32 random bytes generated on the first start, kept in the OS keyring: Secret Service on Linux (GNOME Keyring, KWallet), Keychain on macOS, Credential Manager on Windows, under the service `signal-tui`.
  To see it on Linux: `secret-tool search service signal-tui`.
- **No keyring** (headless Linux, SSH): a passphrase is asked at startup instead, or taken from `SIGNAL_TUI_PASSPHRASE`.
- **Files** are created readable by you only (Unix).
- **Reset**: `signal-tui --reset` deletes the store and its keyring key, then links again. Also remove the old device from your phone (*Settings > Linked devices*): that is what revokes it. Losing the key means resetting.
- **Uninstall**: `signal-tui --uninstall` deletes the store, its keyring key, the read marks and pins.
- **Limits**: a program running in your session can read the unlocked keyring. Full disk encryption protects the rest of your machine.

## Dev

```sh
cargo run --example link_device   # link, then print chats as text
cargo test                        # add `-- --ignored` for the network test
```

## License

[AGPL-3.0-only](LICENSE), like [presage](https://github.com/whisperfish/presage) and [libsignal](https://github.com/signalapp/libsignal) it is built on.
