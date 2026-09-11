# term-markdown

A terminal markdown viewer with syntax-highlighted code blocks and link navigation, built with [ratatui](https://ratatui.rs) + [crossterm](https://github.com/crossterm-rs/crossterm).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/rpmoore/term-markdown/main/scripts/install.sh | bash
```

Downloads the latest release for your platform (Linux x86_64, macOS arm64/x86_64), verifies its checksum, and installs `term-markdown` to `~/.local/bin` — no root/sudo needed. See `scripts/install.sh --help` for options (pinning a version, a different install directory, non-interactive installs).

Or build from source:

```sh
cargo build --release
```

## Usage

```sh
term-markdown <file.md>
```

| Key | Action |
|---|---|
| `q`, `Esc` | quit |
| `j`/`k`, `↓`/`↑` | scroll |
| `d`/`u`, `PageDown`/`PageUp` | half-page scroll |
| `g`/`G`, `Home`/`End` | jump to top/bottom |
| `Tab` / `Shift+Tab` | select next/previous link |
| `Enter` | follow selected link |
| `Backspace` | go back |
| mouse click | follow a link |

## Development

See `AGENTS.md` and `RUST.md` for contributor/agent guidance, and `docs/knowledge/` for how the codebase works.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

Apache-2.0 — see [LICENSE](LICENSE).
