---
type: concept
title: TUI app loop
description: Terminal lifecycle, draw loop, scroll state, and key bindings for the viewer.
resource: src/main.rs
tags: [tui, ratatui, crossterm]
---

# TUI app loop

## Lifecycle

`main` (`src/main.rs:58-66`) parses CLI args via `clap` (`Args { file: PathBuf }`, `src/main.rs:22-27`), builds `App` (which eagerly reads and renders the file — `src/main.rs:36-44`), enters the terminal (`setup_terminal`, `src/main.rs:68-73`: raw mode + alt screen), runs the event loop, then unconditionally restores the terminal (`restore_terminal`, `src/main.rs:75-80`: disable raw mode, leave alt screen, show cursor) before propagating `run`'s result. Restoration runs even if `run` returns `Err`, since it's called on the line after `run` rather than inside a `?`-chained expression — this prevents leaving the user's terminal in raw/alt-screen mode on error.

## App state

`App` (`src/main.rs:29-33`) holds `title` (the file path, used as both window title and status-bar label), `body: Text<'static>` (pre-rendered by `markdown::render`, not re-rendered per frame), and `scroll: u16` (current top line offset).

`max_scroll` (`src/main.rs:46-49`) clamps scrolling to `total_lines - viewport_height` (saturating, so it's `0` once content fits the viewport). `scroll_by` (`src/main.rs:51-55`) applies a signed delta and clamps into `[0, max_scroll]`; callers pass `i32::MIN/2` / `i32::MAX/2` for "jump to top/bottom" (`src/main.rs:129-130`) rather than `i32::MIN/MAX` directly, avoiding overflow when the delta is later added to `scroll as i32`.

## Draw loop

`run` (`src/main.rs:82-138`) loops: draw a frame, then poll for input with a 250ms timeout (`src/main.rs:114`) so the loop stays responsive without busy-waiting. Layout is two rows — `Constraint::Min(1)` body + `Constraint::Length(1)` status bar (`src/main.rs:87-88`). `body_height` is recomputed every frame from the actual rendered chunk height minus 2 (border rows) (`src/main.rs:90`) and captured via closure into the outer scope so the event-handling code below can use the same value for scroll clamping — this means scroll math always reflects the last-drawn frame's size, not the current terminal size if a resize event hasn't been drawn yet.

The body is a bordered `Paragraph` wrapping `app.body.clone()` with `Wrap { trim: false }` and `.scroll((app.scroll, 0))` (`src/main.rs:92-99`) — `app.body` is cloned every frame since `Paragraph::new` takes ownership.

## Key bindings

Only `KeyEventKind::Press` is handled (`src/main.rs:116-118`), which matters on Windows/some terminals that also emit `Release`/`Repeat` key events under crossterm's enhanced keyboard protocol — without this filter those would double-trigger scroll actions.

| Key | Action |
|---|---|
| `q`, `Esc` | quit (`src/main.rs:120`) |
| `j`, `Down` | scroll +1 line (`src/main.rs:121`) |
| `k`, `Up` | scroll -1 line (`src/main.rs:122`) |
| `d`, `PageDown` | scroll +half viewport (`src/main.rs:123-125`) |
| `u`, `PageUp` | scroll -half viewport (`src/main.rs:126-128`) |
| `g`, `Home` | jump to top (`src/main.rs:129`) |
| `G`, `End` | jump to bottom (`src/main.rs:130`) |

## Known gap

No file-watch/reload — `App::new` reads the file once at startup; external edits to the markdown file aren't picked up without restarting.
